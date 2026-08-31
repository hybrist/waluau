use std::sync::Arc;
struct CompilerTimer {
    #[cfg(not(target_family = "wasm"))]
    started: std::time::Instant,
}

impl CompilerTimer {
    fn start() -> Self {
        Self {
            #[cfg(not(target_family = "wasm"))]
            started: std::time::Instant::now(),
        }
    }

    fn elapsed(&self) -> std::time::Duration {
        #[cfg(not(target_family = "wasm"))]
        return self.started.elapsed();
        #[cfg(target_family = "wasm")]
        return std::time::Duration::ZERO;
    }

    fn enabled() -> bool {
        #[cfg(not(target_family = "wasm"))]
        return std::env::var_os("WALUAU_TIMINGS").is_some();
        #[cfg(target_family = "wasm")]
        return false;
    }
}

#[derive(Default)]
pub struct BuildCache {
    resolved: Option<Program>,
    erased: Option<Program>,
    monomorphic: Option<Program>,
    module: Option<Module>,
    last_incremental: bool,
}

pub fn build(program: &Program) -> Result<Module, Diagnostic> {
    build_inner(program, None, None)
}

fn collect_referenced_symbols_from_expr(expr: &Expr, symbols: &mut HashSet<SymbolId>) {
    match expr {
        Expr::Name(_, Some(symbol_id), _) => {
            symbols.insert(*symbol_id);
        }
        Expr::Unary { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::IsVariant { expr, .. }
        | Expr::Field { base: expr, .. } => collect_referenced_symbols_from_expr(expr, symbols),
        Expr::Binary { left, right, .. } | Expr::Index { base: left, index: right, .. } => {
            collect_referenced_symbols_from_expr(left, symbols);
            collect_referenced_symbols_from_expr(right, symbols);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            collect_referenced_symbols_from_expr(condition, symbols);
            collect_referenced_symbols_from_expr(then_expr, symbols);
            collect_referenced_symbols_from_expr(else_expr, symbols);
        }
        Expr::Call {
            callee,
            args,
            method_call_origin,
            ..
        } => {
            collect_referenced_symbols_from_expr(callee, symbols);
            for arg in args {
                collect_referenced_symbols_from_expr(arg, symbols);
            }
            if let Some(origin) = method_call_origin {
                collect_referenced_symbols_from_expr(&origin.original_receiver, symbols);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_referenced_symbols_from_expr(receiver, symbols);
            for arg in args {
                collect_referenced_symbols_from_expr(arg, symbols);
            }
        }
        Expr::Function(function) => {
            collect_referenced_symbols_from_stmts(&function.body, symbols);
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                collect_referenced_symbols_from_expr(element, symbols);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                collect_referenced_symbols_from_expr(&field.value, symbols);
            }
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Name(_, None, _)
        | Expr::Vararg(_)
        | Expr::Require(..) => {}
    }
}

fn collect_referenced_symbols_from_stmts(stmts: &[Stmt], symbols: &mut HashSet<SymbolId>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Expr(value) | Stmt::Return(value) => {
                collect_referenced_symbols_from_expr(value, symbols);
            }
            Stmt::Assign {
                symbol_id, value, ..
            } => {
                if let Some(symbol_id) = symbol_id {
                    symbols.insert(*symbol_id);
                }
                collect_referenced_symbols_from_expr(value, symbols);
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                collect_referenced_symbols_from_expr(base, symbols);
                collect_referenced_symbols_from_expr(index, symbols);
                collect_referenced_symbols_from_expr(value, symbols);
            }
            Stmt::FieldAssign { base, value, .. } => {
                collect_referenced_symbols_from_expr(base, symbols);
                collect_referenced_symbols_from_expr(value, symbols);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_referenced_symbols_from_expr(condition, symbols);
                collect_referenced_symbols_from_stmts(then_body, symbols);
                collect_referenced_symbols_from_stmts(else_body, symbols);
            }
            Stmt::IfCast {
                value,
                then_body,
                else_body,
                ..
            } => {
                collect_referenced_symbols_from_expr(value, symbols);
                collect_referenced_symbols_from_stmts(then_body, symbols);
                collect_referenced_symbols_from_stmts(else_body, symbols);
            }
            Stmt::Match { value, arms, .. } => {
                collect_referenced_symbols_from_expr(value, symbols);
                for arm in arms {
                    collect_referenced_symbols_from_stmts(&arm.body, symbols);
                }
            }
            Stmt::While { condition, body } => {
                collect_referenced_symbols_from_expr(condition, symbols);
                collect_referenced_symbols_from_stmts(body, symbols);
            }
            Stmt::Repeat { body, condition } => {
                collect_referenced_symbols_from_stmts(body, symbols);
                collect_referenced_symbols_from_expr(condition, symbols);
            }
            Stmt::NumericFor {
                start,
                stop,
                step,
                body,
                ..
            } => {
                collect_referenced_symbols_from_expr(start, symbols);
                collect_referenced_symbols_from_expr(stop, symbols);
                if let Some(step) = step {
                    collect_referenced_symbols_from_expr(step, symbols);
                }
                collect_referenced_symbols_from_stmts(body, symbols);
            }
            Stmt::ForIn {
                iterators, body, ..
            } => {
                for iterator in iterators {
                    collect_referenced_symbols_from_expr(iterator, symbols);
                }
                collect_referenced_symbols_from_stmts(body, symbols);
            }
            Stmt::ReturnMulti(values) | Stmt::LetMulti { values, .. } => {
                for value in values {
                    collect_referenced_symbols_from_expr(value, symbols);
                }
            }
            Stmt::AssignMulti {
                symbol_ids, values, ..
            } => {
                if let Some(symbol_ids) = symbol_ids {
                    symbols.extend(symbol_ids.iter().copied());
                }
                for value in values {
                    collect_referenced_symbols_from_expr(value, symbols);
                }
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn collect_module_globals(program: &Program) -> Result<Vec<Global>, Diagnostic> {
    let Some(init) = program
        .functions
        .iter()
        .find(|function| function.name.to_string() == "__waluau_top_level_init")
    else {
        return Ok(Vec::new());
    };
    let mut referenced = HashSet::new();
    for function in &program.functions {
        if function.name.to_string() != "__waluau_top_level_init" {
            collect_referenced_symbols_from_stmts(&function.body, &mut referenced);
        }
    }
    let mut globals = Vec::new();
    for stmt in &init.body {
        match stmt {
            Stmt::Let {
                name,
                symbol_id,
                ty,
                ..
            } => {
                if let Some(symbol_id) = symbol_id
                    && referenced.contains(symbol_id)
                {
                    let ty = ty.clone().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "module binding '{name}' must have a concrete type"
                        ))
                    })?;
                    globals.push(Global {
                        name: name.clone(),
                        symbol_id: *symbol_id,
                        ty,
                    });
                }
            }
            Stmt::LetMulti { bindings, .. } => {
                for binding in bindings {
                    if let Some(symbol_id) = binding.symbol_id
                        && referenced.contains(&symbol_id)
                    {
                        let ty = binding.ty.clone().ok_or_else(|| {
                            Diagnostic::new(format!(
                                "module binding '{}' must have a concrete type",
                                binding.name
                            ))
                        })?;
                        globals.push(Global {
                            name: binding.name.clone(),
                            symbol_id,
                            ty,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    Ok(globals)
}

pub fn build_cached<'a>(
    program: &Program,
    cache: &'a mut BuildCache,
) -> Result<&'a Module, Diagnostic> {
    build_cached_with_changes(program, cache, &[])
}

pub fn build_cached_with_changes<'a>(
    program: &Program,
    cache: &'a mut BuildCache,
    changed_functions: &[usize],
) -> Result<&'a Module, Diagnostic> {
    cache.last_incremental = false;
    let changed_hint = (changed_functions.len() == 1).then_some(changed_functions);
    let module = build_inner(program, Some(cache), changed_hint)?;
    cache.module = Some(module);
    Ok(cache.module.as_ref().expect("cached IR module"))
}

impl BuildCache {
    pub fn last_build_was_incremental(&self) -> bool {
        self.last_incremental
    }
}

/// Temporary diagnostics for byte-determinism debugging: with
/// WALUAU_STAGE_HASH set, print a hash of each pipeline stage's Debug form.
fn stage_hash(stage: &str, value: &impl std::fmt::Debug) {
    if std::env::var_os("WALUAU_STAGE_HASH").is_none() {
        return;
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // DefaultHasher::new() is seed-stable across processes.
    format!("{value:?}").hash(&mut hasher);
    eprintln!("stage {stage}: {:016x}", hasher.finish());
}

fn build_inner(
    program: &Program,
    mut cache: Option<&mut BuildCache>,
    changed_hint: Option<&[usize]>,
) -> Result<Module, Diagnostic> {
    let started = CompilerTimer::start();
    if let Some(function) = program.functions.iter().find(|function| {
        function.declaration_class == waluau_ast::FunctionDeclarationClass::Export
            && function.name.unqualified_name().is_some_and(|name| {
                name.starts_with("__waluau_") || name == "memory"
            })
    }) {
        return Err(Diagnostic::new(format!(
            "browser-exported function '{}' uses a compiler-owned export name; names beginning '__waluau_' and 'memory' are reserved",
            function.name
        )));
    }
    if let Some(function) = program.functions.iter().find(|function| {
        function.declaration_class == waluau_ast::FunctionDeclarationClass::Export
            && !function.type_params.is_empty()
    }) {
        return Err(Diagnostic::new(format!(
            "browser-exported function '{}' cannot be generic",
            function.name
        )));
    }
    let hinted_index = changed_hint
        .and_then(|changed| changed.first().copied())
        .filter(|index| {
            program
                .functions
                .get(*index)
                .is_some_and(|function| function.type_params.is_empty())
        });
    let mut resolved = if let Some(index) = hinted_index
        && let Some(mut resolved) = cache.as_deref_mut().and_then(|cache| cache.resolved.take())
        && resolved.functions.len() == program.functions.len()
    {
        resolved.functions[index] = program.functions[index].clone();
        resolved.sources = program.sources.clone();
        resolved
    } else {
        program.clone()
    };
    waluau_ast::resolve_symbols(&mut resolved)?;
    let tooling_function_exports = collect_tooling_function_exports(&resolved)?;
    stage_hash("resolved", &resolved);
    let resolved_at = started.elapsed();
    let incremental_index = hinted_index.or_else(|| cache.as_deref().and_then(|cache| {
        let previous = cache.resolved.as_ref()?;
        if !incremental_context_matches(&resolved, previous) {
            return None;
        }
        let changed = resolved
            .functions
            .iter()
            .zip(&previous.functions)
            .enumerate()
            .filter_map(|(index, (current, previous))| (current != previous).then_some(index))
            .collect::<Vec<_>>();
        (changed.len() == 1 && resolved.functions[changed[0]].type_params.is_empty())
            .then(|| changed[0])
    }));
    let erased = if let Some(index) = incremental_index
        && let Some(mut erased) = cache.as_deref_mut().and_then(|cache| cache.erased.take())
    {
        erased.functions[index] = erase_function_opaque_types(&resolved.functions[index]);
        erased.sources = resolved.sources.clone();
        erased
    } else {
        erase_opaque_types(&resolved)
    };
    stage_hash("erased", &erased);
    let erased_at = started.elapsed();
    if let Some(cached) = cache.as_deref_mut()
        && let Some((monomorphic, module)) =
            try_build_incremental(&resolved, &erased, cached, incremental_index)?
    {
        cached.resolved = Some(resolved);
        cached.erased = Some(erased);
        cached.monomorphic = Some(monomorphic);
        cached.last_incremental = true;
        if CompilerTimer::enabled() {
            eprintln!(
                "waluau ir timings: symbols={:?} erase={:?} incremental={:?}",
                resolved_at,
                erased_at - resolved_at,
                started.elapsed() - erased_at,
            );
        }
        return Ok(module);
    }
    let mut monomorphic = Monomorphizer::new(&erased).run(&erased)?;
    // Monomorphization already resolved the specialized program; re-resolving
    // is deterministic and re-stamps the same ids, so this pass only exists to
    // capture the symbol-name table for Wasm debug-name emission.
    let symbol_names = waluau_ast::resolve_symbols(&mut monomorphic)?;
    stage_hash("monomorphic", &monomorphic);
    let monomorphic_at = started.elapsed();
    let globals = collect_module_globals(&monomorphic)?;
    let global_indices = globals
        .iter()
        .enumerate()
        .map(|(index, global)| (global.symbol_id, (index, global.ty.clone())))
        .collect::<HashMap<_, _>>();
    let tag_ids = collect_variant_tag_ids(&monomorphic, &resolved.type_declarations);
    let source_files = collect_source_files(&monomorphic.sources);
    let source_file_ids = source_files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.path.clone(), SourceFileId(index as u32)))
        .collect::<HashMap<_, _>>();
    let mut signatures = HashMap::new();
    let mut field_call_signatures = HashMap::new();
    let mut declared_imports = Vec::new();
    for declared in &monomorphic.declared_imports {
        let symbol_id = declared.symbol_id.ok_or_else(|| {
            Diagnostic::new(format!(
                "declared host function '{}' must have a symbol ID resolved",
                declared.name
            ))
        })?;
        let sig = (
            declared
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect::<Vec<_>>(),
            declared.return_type.clone(),
        );
        signatures.insert(symbol_id, sig.clone());
        field_call_signatures.insert(declared.name.clone(), sig);
        let host_name = canonical_dom_import_host_name(&declared.host_name)
            .unwrap_or_else(|| declared.host_name.clone());
        declared_imports.push(DeclaredImport {
            module: "waluau".to_string(),
            name: declared.name.clone(),
            host_name,
            params: declared
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect(),
            return_type: declared.return_type.clone(),
            symbol_id,
        });
    }
    for function in &monomorphic.functions {
        let symbol_id = function.symbol_id.ok_or_else(|| {
            Diagnostic::new(format!(
                "function '{}' must have a symbol ID resolved",
                function.name
            ))
        })?;
        let return_type = function.return_type.clone().ok_or_else(|| {
            Diagnostic::new(format!(
                "function '{}' must have a concrete return type before IR lowering",
                function.name
            ))
        })?;
        let mut param_types = function
            .params
            .iter()
            .map(|param| param.ty.clone())
            .collect::<Vec<_>>();
        if function.vararg.is_some() {
            param_types.push(Type::Variadic(Arc::new(Type::Unknown)));
        }
        let sig = (param_types, return_type);
        signatures.insert(symbol_id, sig.clone());
        field_call_signatures.insert(function.name.to_string(), sig);
    }
    let host_import_signatures = declared_imports
        .iter()
        .map(|declared| {
            (
                declared.symbol_id,
                (declared.params.clone(), declared.return_type.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    let host_import_names = declared_imports
        .iter()
        .map(|declared| (declared.name.clone(), declared.symbol_id))
        .collect::<HashMap<_, _>>();
    let declared_constants = monomorphic
        .declared_constants
        .iter()
        .map(|constant| {
            (
                constant.name.clone(),
                (constant.ty.clone(), constant.value.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    let cx = ModuleLoweringContext {
        signatures: &signatures,
        host_import_signatures: &host_import_signatures,
        host_import_names: &host_import_names,
        field_call_signatures: &field_call_signatures,
        declared_constants: &declared_constants,
        globals: &global_indices,
        sources: &monomorphic.sources,
        source_file_ids: &source_file_ids,
        tag_ids: &tag_ids,
    };
    #[cfg(target_family = "wasm")]
    let lowered_functions = monomorphic
        .functions
        .iter()
        .map(|function| build_function(function, &cx))
        .collect::<Vec<_>>();
    #[cfg(not(target_family = "wasm"))]
    let lowered_functions = std::thread::scope(|scope| {
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(monomorphic.functions.len().max(1));
        let chunk_size = monomorphic.functions.len().max(1).div_ceil(workers);
        let handles = monomorphic
            .functions
            .chunks(chunk_size)
            .map(|chunk| {
                let cx = &cx;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|function| build_function(function, cx))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("IR lowering worker panicked"))
            .collect::<Vec<_>>()
    });
    let mut functions = Vec::new();
    for lowered in lowered_functions {
        functions.extend(lowered?);
    }
    let lowered_at = started.elapsed();
    let authored_function_exports = collect_authored_function_exports(&monomorphic)?;
    let start = functions
        .iter()
        .position(|function| function.name == "__waluau_top_level_init");

    // If the top-level init function uses promise await, it must run inside a
    // coroutine (await requires an active coroutine state). Auto-wrap it: rename
    // the body to `$async_body`, then replace it with a thin coroutine wrapper.
    let (functions, start) = wrap_async_top_level_init_if_needed(functions, start);

    let module = Module {
        functions,
        tooling_function_exports,
        authored_function_exports,
        globals,
        declared_imports,
        start,
        tag_ids,
        symbol_names,
        source_files,
    };
    if CompilerTimer::enabled() {
        eprintln!(
            "waluau ir timings: symbols={:?} erase={:?} mono={:?} setup+lower={:?} finish={:?}",
            resolved_at,
            erased_at - resolved_at,
            monomorphic_at - erased_at,
            lowered_at - monomorphic_at,
            started.elapsed() - lowered_at,
        );
    }
    verify(&module)?;
    if let Some(cached) = cache {
        cached.resolved = Some(resolved);
        cached.erased = Some(erased);
        cached.monomorphic = Some(monomorphic);
    }
    stage_hash("module", &module);
    if let Some(dump) = std::env::var_os("WALUAU_DUMP_FN") {
        let wanted = dump.to_string_lossy();
        for function in &module.functions {
            if function.name.contains(wanted.as_ref()) {
                eprintln!("=== {} ===\n{function:#?}", function.name);
            }
        }
    }
    Ok(module)
}

fn incremental_context_matches(current: &Program, previous: &Program) -> bool {
    current.declared_imports == previous.declared_imports
        && current.declared_constants == previous.declared_constants
        && current.type_declarations == previous.type_declarations
        && current.top_level == previous.top_level
        && current.export == previous.export
        && current.entry_file_path == previous.entry_file_path
        && current.functions.len() == previous.functions.len()
        && current
            .functions
            .iter()
            .zip(&previous.functions)
            .all(|(current, previous)| {
                current.name == previous.name
                    && current.declaration_class == previous.declaration_class
                    && current.symbol_id == previous.symbol_id
                    && current.type_params == previous.type_params
                    && current.params == previous.params
                    && current.vararg == previous.vararg
                    && current.return_type == previous.return_type
                    && current.file_path == previous.file_path
            })
}

fn collect_source_files(sources: &BTreeMap<String, String>) -> Vec<SourceFile> {
    assert!(
        u32::try_from(sources.len()).is_ok(),
        "source file table exceeds u32 indices"
    );
    sources
        .iter()
        .map(|(path, source)| SourceFile {
            path: path.clone(),
            source: source.clone(),
        })
        .collect()
}

fn collect_authored_function_exports(
    program: &Program,
) -> Result<BTreeMap<SymbolId, String>, Diagnostic> {
    program
        .functions
        .iter()
        .filter(|function| {
            function.declaration_class == waluau_ast::FunctionDeclarationClass::Export
        })
        .map(|function| {
            let symbol_id = function.symbol_id.ok_or_else(|| {
                Diagnostic::new(format!(
                    "exported function '{}' must have a symbol ID resolved",
                    function.name
                ))
            })?;
            Ok((symbol_id, function.name.to_string()))
        })
        .collect()
}

fn collect_tooling_function_exports(
    program: &Program,
) -> Result<BTreeMap<SymbolId, String>, Diagnostic> {
    program
        .functions
        .iter()
        .filter(|function| {
            function.declaration_class().facts().binding
                == waluau_ast::FunctionBindingClass::Module
                && function.type_params.is_empty()
                && function.is_authored_declaration()
                && function.file_path == program.entry_file_path
        })
        .map(|function| {
            let symbol_id = function.symbol_id.ok_or_else(|| {
                Diagnostic::new(format!(
                    "authored module function '{}' must have a symbol ID resolved",
                    function.name
                ))
            })?;
            Ok((symbol_id, function.name.to_string()))
        })
        .collect()
}

/// Rebuild one ordinary function while preserving the previously verified IR
/// for every other function. Changes that can alter specialization or lifted
/// function shape deliberately fall back to the full pipeline.
fn try_build_incremental(
    resolved: &Program,
    erased: &Program,
    cache: &mut BuildCache,
    changed_hint: Option<usize>,
) -> Result<Option<(Program, Module)>, Diagnostic> {
    let started = CompilerTimer::start();
    let (Some(previous_monomorphic), Some(previous_module)) =
        (&cache.monomorphic, &cache.module)
    else {
        return Ok(None);
    };
    let Some(changed_index) = changed_hint else {
        return Ok(None);
    };

    let changed_function = &erased.functions[changed_index];
    let mut monomorphizer = Monomorphizer::new(erased);
    let rewritten = monomorphizer.run_function(changed_function)?;
    let rewritten_at = started.elapsed();
    if rewritten.len() != 1 {
        return Ok(None);
    }
    let rewritten = rewritten.into_iter().next().expect("one rewritten function");
    let name = rewritten.name.to_string();
    let Some(monomorphic_index) = previous_monomorphic
        .functions
        .iter()
        .position(|function| function.name.to_string() == name)
    else {
        return Ok(None);
    };

    let mut monomorphic = cache
        .monomorphic
        .take()
        .expect("cached monomorphic program");
    monomorphic.functions[monomorphic_index] = rewritten;
    monomorphic.sources = erased.sources.clone();
    let symbol_names = waluau_ast::resolve_symbols(&mut monomorphic)?;
    let resolved_at = started.elapsed();

    let tag_ids = collect_variant_tag_ids(&monomorphic, &resolved.type_declarations);
    if tag_ids != previous_module.tag_ids {
        return Ok(None);
    }
    let mut signatures = HashMap::new();
    let mut field_call_signatures = HashMap::new();
    let mut declared_imports = Vec::new();
    for declared in &monomorphic.declared_imports {
        let symbol_id = declared.symbol_id.ok_or_else(|| {
            Diagnostic::new(format!(
                "declared host function '{}' must have a symbol ID resolved",
                declared.name
            ))
        })?;
        let sig = (
            declared.params.iter().map(|param| param.ty.clone()).collect(),
            declared.return_type.clone(),
        );
        signatures.insert(symbol_id, sig.clone());
        field_call_signatures.insert(declared.name.clone(), sig);
        declared_imports.push(DeclaredImport {
            module: "waluau".to_string(),
            name: declared.name.clone(),
            host_name: canonical_dom_import_host_name(&declared.host_name)
                .unwrap_or_else(|| declared.host_name.clone()),
            params: declared.params.iter().map(|param| param.ty.clone()).collect(),
            return_type: declared.return_type.clone(),
            symbol_id,
        });
    }
    for function in &monomorphic.functions {
        let symbol_id = function.symbol_id.ok_or_else(|| {
            Diagnostic::new(format!(
                "function '{}' must have a symbol ID resolved",
                function.name
            ))
        })?;
        let return_type = function.return_type.clone().ok_or_else(|| {
            Diagnostic::new(format!(
                "function '{}' must have a concrete return type before IR lowering",
                function.name
            ))
        })?;
        let mut params = function.params.iter().map(|param| param.ty.clone()).collect::<Vec<_>>();
        if function.vararg.is_some() {
            params.push(Type::Variadic(Arc::new(Type::Unknown)));
        }
        let sig = (params, return_type);
        signatures.insert(symbol_id, sig.clone());
        field_call_signatures.insert(function.name.to_string(), sig);
    }
    let host_import_signatures = declared_imports
        .iter()
        .map(|declared| {
            (
                declared.symbol_id,
                (declared.params.clone(), declared.return_type.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    let host_import_names = declared_imports
        .iter()
        .map(|declared| (declared.name.clone(), declared.symbol_id))
        .collect::<HashMap<_, _>>();
    let declared_constants = monomorphic
        .declared_constants
        .iter()
        .map(|constant| {
            (
                constant.name.clone(),
                (constant.ty.clone(), constant.value.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    let globals = collect_module_globals(&monomorphic)?;
    if globals != previous_module.globals {
        return Ok(None);
    }
    let global_indices = globals
        .iter()
        .enumerate()
        .map(|(index, global)| (global.symbol_id, (index, global.ty.clone())))
        .collect::<HashMap<_, _>>();
    let source_files = collect_source_files(&monomorphic.sources);
    if source_files
        .iter()
        .map(|file| &file.path)
        .ne(previous_module.source_files.iter().map(|file| &file.path))
    {
        return Ok(None);
    }
    let source_file_ids = source_files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.path.clone(), SourceFileId(index as u32)))
        .collect::<HashMap<_, _>>();
    let cx = ModuleLoweringContext {
        signatures: &signatures,
        host_import_signatures: &host_import_signatures,
        host_import_names: &host_import_names,
        field_call_signatures: &field_call_signatures,
        declared_constants: &declared_constants,
        globals: &global_indices,
        sources: &monomorphic.sources,
        source_file_ids: &source_file_ids,
        tag_ids: &tag_ids,
    };
    let lowered = build_function(&monomorphic.functions[monomorphic_index], &cx)?;
    let lowered_at = started.elapsed();
    if lowered.len() != 1 || lowered[0].name != name {
        return Ok(None);
    }
    if declared_imports != previous_module.declared_imports {
        return Ok(None);
    }
    if collect_authored_function_exports(&monomorphic)?
        != previous_module.authored_function_exports
    {
        return Ok(None);
    }
    if collect_tooling_function_exports(resolved)? != previous_module.tooling_function_exports {
        return Ok(None);
    }
    let Some(module_index) = previous_module
        .functions
        .iter()
        .position(|function| function.name == name)
    else {
        return Ok(None);
    };
    if lowered[0].params != previous_module.functions[module_index].params
        || lowered[0].return_type != previous_module.functions[module_index].return_type
    {
        return Ok(None);
    }
    let mut module = cache.module.take().expect("cached IR module");
    module.functions[module_index] = lowered.into_iter().next().expect("one lowered function");
    // The relowered function carries symbol ids from the fresh resolution, so
    // the module-wide debug-name table must come from that resolution too.
    // Unchanged functions keep their old ids; if the edit shifted ids their
    // debug names can go stale until the next full build, which is acceptable
    // for name-section metadata.
    module.symbol_names = symbol_names;
    module.source_files = source_files;
    let names = HashSet::from([name]);
    verify::verify_functions(&module, &names)?;
    if CompilerTimer::enabled() {
        eprintln!(
            "waluau ir incremental detail: rewrite={:?} clone+symbols={:?} setup+lower={:?} clone+verify={:?}",
            rewritten_at,
            resolved_at - rewritten_at,
            lowered_at - resolved_at,
            started.elapsed() - lowered_at,
        );
    }
    Ok(Some((monomorphic, module)))
}

fn wrap_async_top_level_init_if_needed(
    mut functions: Vec<Function>,
    start: Option<usize>,
) -> (Vec<Function>, Option<usize>) {
    let Some(start_idx) = start else {
        return (functions, start);
    };
    let has_await = functions[start_idx].blocks.values().any(|block| {
        matches!(block.terminator, Terminator::CoroutineAwaitPromise { .. })
    });
    if !has_await {
        return (functions, start);
    }
    let async_body_name = "__waluau_top_level_init$async_body".to_string();
    functions[start_idx].name = async_body_name.clone();

    let entry = BlockId(0);
    let v_closure = ValueId(0);
    let v_coroutine = ValueId(1);
    let v_resume = ValueId(2);
    let v_unit = ValueId(3);
    let mut blocks = std::collections::BTreeMap::new();
    blocks.insert(
        entry,
        BasicBlock {
            id: entry,
            instructions: vec![
                (
                    v_closure,
                    Instruction::Closure {
                        name: async_body_name,
                        captures: vec![],
                        params: vec![],
                        return_type: Type::Numeric(NumericType::I32),
                    },
                ),
                (v_coroutine, Instruction::CoroutineCreate { callee: v_closure }),
                (v_resume, Instruction::CoroutineResume { coroutine: v_coroutine }),
                (v_unit, Instruction::Unit),
            ],
            terminator: Terminator::Return(v_unit),
        },
    );
    let new_start = functions.len();
    functions.push(Function {
        name: "__waluau_top_level_init".to_string(),
        params: vec![],
        return_type: Type::Unit,
        entry,
        blocks,
        next_value: 4,
        capture_count: 0,
        value_symbols: std::collections::BTreeMap::new(),
        symbol_id: None,
        source_map: FunctionSourceMap::synthetic(),
    });
    (functions, Some(new_start))
}

fn canonical_dom_import_host_name(name: &str) -> Option<String> {
    let (interface, member) = name.split_once('.')?;
    if member.starts_with("__") {
        return None;
    }
    let host_member = if let Some(property) = member.strip_prefix("get/") {
        if !property.contains('_') {
            return None;
        }
        format!("get/{}", snake_to_lower_camel(property))
    } else if let Some(property) = member.strip_prefix("set/") {
        if !property.contains('_') {
            return None;
        }
        format!("set/{}", snake_to_lower_camel(property))
    } else if member.contains('_') {
        snake_to_lower_camel(member)
    } else {
        return None;
    };
    Some(format!("{interface}.{host_member}"))
}

fn snake_to_lower_camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut uppercase_next = false;
    for ch in name.chars() {
        if ch == '_' {
            uppercase_next = true;
        } else if uppercase_next {
            out.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

struct IfCastParts<'a> {
    target_name: &'a str,
    binding_symbol_id: Option<SymbolId>,
    value: &'a Expr,
    then_body: &'a [Stmt],
    else_body: &'a [Stmt],
}

fn collect_variant_tag_ids(program: &Program, type_declarations: &[TypeDeclaration]) -> BTreeMap<String, i32> {
    let mut tag_ids = BTreeMap::new();
    for decl in type_declarations {
        collect_type_variant_tags(&decl.ty, &mut tag_ids);
    }
    for function in &program.functions {
        for param in &function.params {
            collect_type_variant_tags(&param.ty, &mut tag_ids);
        }
        if let Some(return_type) = &function.return_type {
            collect_type_variant_tags(return_type, &mut tag_ids);
        }
        for stmt in &function.body {
            collect_stmt_variant_tags(stmt, &mut tag_ids);
        }
    }
    for stmt in &program.top_level {
        collect_stmt_variant_tags(stmt, &mut tag_ids);
    }
    // Tagged coroutine resume can produce all three runtime variants even when
    // a source annotation only models a subset such as `Finished | Error`.
    insert_variant_tag(&mut tag_ids, "Yielded");
    insert_variant_tag(&mut tag_ids, "Finished");
    insert_variant_tag(&mut tag_ids, "Error");
    tag_ids
}

fn insert_variant_tag(tag_ids: &mut BTreeMap<String, i32>, tag: &str) {
    if !tag_ids.contains_key(tag) {
        tag_ids.insert(tag.to_string(), tag_ids.len() as i32);
    }
}

fn collect_type_variant_tags(ty: &Type, tag_ids: &mut BTreeMap<String, i32>) {
    match ty {
        Type::TaggedVariant(variant) => {
            insert_variant_tag(tag_ids, &variant.tag);
            collect_type_variant_tags(variant.payload.as_ref(), tag_ids);
        }
        Type::TaggedUnion(variants) => {
            for variant in variants {
                insert_variant_tag(tag_ids, &variant.tag);
                collect_type_variant_tags(variant.payload.as_ref(), tag_ids);
            }
        }
        Type::Opaque { ty, .. } => collect_type_variant_tags(ty, tag_ids),
        Type::Array(ty)
        | Type::Variadic(ty)
        | Type::Nullable(ty)
        | Type::ExternSubtype(ty) => {
            collect_type_variant_tags(ty, tag_ids)
        }
        Type::Multi(types) => {
            for ty in types {
                collect_type_variant_tags(ty, tag_ids);
            }
        }
        Type::Function {
            params,
            return_type,
            ..
        } => {
            for param in params {
                collect_type_variant_tags(param, tag_ids);
            }
            collect_type_variant_tags(return_type, tag_ids);
        }
        Type::Record(fields) => {
            for ty in fields.values() {
                collect_type_variant_tags(ty, tag_ids);
            }
        }
        Type::Named { type_args, .. } => {
            for ty in type_args {
                collect_type_variant_tags(ty, tag_ids);
            }
        }
        Type::Unit
        | Type::Bool
        | Type::Numeric(_)
        | Type::String
        | Type::Bytes
        | Type::Buffer
        | Type::Extern
        | Type::Nil
        | Type::Unknown
        | Type::Thread
        | Type::TypedArray(_)
        | Type::TypeParam(_)
        | Type::StringLiteralUnion(_) => {}
    }
}

fn collect_stmt_variant_tags(stmt: &Stmt, tag_ids: &mut BTreeMap<String, i32>) {
    match stmt {
        Stmt::Let { ty, value, .. } => {
            if let Some(ty) = ty {
                collect_type_variant_tags(ty, tag_ids);
            }
            collect_expr_variant_tags(value, tag_ids);
        }
        Stmt::Assign { value, .. } | Stmt::Return(value) | Stmt::Expr(value) => {
            collect_expr_variant_tags(value, tag_ids);
        }
        Stmt::IndexAssign {
            base,
            index,
            value,
            ..
        } => {
            collect_expr_variant_tags(base, tag_ids);
            collect_expr_variant_tags(index, tag_ids);
            collect_expr_variant_tags(value, tag_ids);
        }
        Stmt::FieldAssign { base, value, .. } => {
            collect_expr_variant_tags(base, tag_ids);
            collect_expr_variant_tags(value, tag_ids);
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            collect_expr_variant_tags(condition, tag_ids);
            for stmt in then_body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
            for stmt in else_body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::IfCast {
            target_ty,
            value,
            then_body,
            else_body,
            ..
        } => {
            collect_type_variant_tags(target_ty, tag_ids);
            collect_expr_variant_tags(value, tag_ids);
            for stmt in then_body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
            for stmt in else_body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::Match {
            value,
            enum_ty,
            arms,
        } => {
            collect_expr_variant_tags(value, tag_ids);
            collect_type_variant_tags(enum_ty, tag_ids);
            for arm in arms {
                for stmt in &arm.body {
                    collect_stmt_variant_tags(stmt, tag_ids);
                }
            }
        }
        Stmt::While { condition, body } => {
            collect_expr_variant_tags(condition, tag_ids);
            for stmt in body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::Repeat { body, condition } => {
            for stmt in body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
            collect_expr_variant_tags(condition, tag_ids);
        }
        Stmt::NumericFor {
            start,
            stop,
            step,
            body,
            ..
        } => {
            collect_expr_variant_tags(start, tag_ids);
            collect_expr_variant_tags(stop, tag_ids);
            if let Some(step) = step {
                collect_expr_variant_tags(step, tag_ids);
            }
            for stmt in body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::ForIn {
            iterators, body, ..
        } => {
            for iterator in iterators {
                collect_expr_variant_tags(iterator, tag_ids);
            }
            for stmt in body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Stmt::ReturnMulti(values) => {
            for value in values {
                collect_expr_variant_tags(value, tag_ids);
            }
        }
        Stmt::LetMulti { bindings, values } => {
            for binding in bindings {
                if let Some(ty) = &binding.ty {
                    collect_type_variant_tags(ty, tag_ids);
                }
            }
            for value in values {
                collect_expr_variant_tags(value, tag_ids);
            }
        }
        Stmt::AssignMulti { values, .. } => {
            for value in values {
                collect_expr_variant_tags(value, tag_ids);
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn collect_expr_variant_tags(expr: &Expr, tag_ids: &mut BTreeMap<String, i32>) {
    match expr {
        Expr::Unary { expr, .. } => {
            collect_expr_variant_tags(expr, tag_ids);
        }
        Expr::Cast { expr, ty, .. } => {
            collect_expr_variant_tags(expr, tag_ids);
            collect_type_variant_tags(ty, tag_ids);
        }
        Expr::IsVariant { expr, .. } => collect_expr_variant_tags(expr, tag_ids),
        Expr::Binary { left, right, .. } => {
            collect_expr_variant_tags(left, tag_ids);
            collect_expr_variant_tags(right, tag_ids);
        }
        Expr::If {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            collect_expr_variant_tags(condition, tag_ids);
            collect_expr_variant_tags(then_expr, tag_ids);
            collect_expr_variant_tags(else_expr, tag_ids);
        }
        Expr::Call {
            callee,
            type_args,
            args,
            ..
        } => {
            collect_expr_variant_tags(callee, tag_ids);
            for ty in type_args {
                collect_type_variant_tags(ty, tag_ids);
            }
            for arg in args {
                collect_expr_variant_tags(arg, tag_ids);
            }
        }
        Expr::MethodCall {
            receiver,
            args,
            type_args,
            ..
        } => {
            collect_expr_variant_tags(receiver, tag_ids);
            for ty in type_args {
                collect_type_variant_tags(ty, tag_ids);
            }
            for arg in args {
                collect_expr_variant_tags(arg, tag_ids);
            }
        }
        Expr::Function(function) => {
            for param in &function.params {
                collect_type_variant_tags(&param.ty, tag_ids);
            }
            if let Some(return_type) = &function.return_type {
                collect_type_variant_tags(return_type, tag_ids);
            }
            for stmt in &function.body {
                collect_stmt_variant_tags(stmt, tag_ids);
            }
        }
        Expr::ArrayLiteral { elements, .. } => {
            for element in elements {
                collect_expr_variant_tags(element, tag_ids);
            }
        }
        Expr::TableLiteral { fields, .. } => {
            for field in fields {
                collect_expr_variant_tags(&field.value, tag_ids);
            }
        }
        Expr::Field { base, .. } => collect_expr_variant_tags(base, tag_ids),
        Expr::Index { base, index, .. } => {
            collect_expr_variant_tags(base, tag_ids);
            collect_expr_variant_tags(index, tag_ids);
        }
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Name(..)
        | Expr::Require(..) => {}
    }
}

fn erase_opaque_types(program: &Program) -> Program {
    Program {
        functions: program.functions.iter().map(erase_function_opaque_types).collect(),
        declared_imports: program
            .declared_imports
            .iter()
            .map(|declared| waluau_ast::DeclaredImport {
                name: declared.name.clone(),
                host_name: declared.host_name.clone(),
                symbol_id: declared.symbol_id,
                params: declared
                    .params
                    .iter()
                    .map(|param| waluau_ast::Param {
                        name: param.name.clone(),
                        symbol_id: param.symbol_id,
                        ty: erase_type_opaque_types(&param.ty),
                    })
                    .collect(),
                return_type: erase_type_opaque_types(&declared.return_type),
            })
            .collect(),
        declared_constants: program.declared_constants.clone(),
        type_declarations: Vec::new(),
        top_level: program.top_level.iter().map(erase_stmt_opaque_types).collect(),
        top_level_file_paths: program.top_level_file_paths.clone(),
        export: program.export.as_ref().map(erase_expr_opaque_types),
        sources: program.sources.clone(),
        entry_file_path: program.entry_file_path.clone(),
    }
}

fn erase_function_opaque_types(function: &AstFunction) -> AstFunction {
    AstFunction {
        name: function.name.clone(),
        declaration_class: function.declaration_class,
        symbol_id: function.symbol_id,
        type_params: function.type_params.clone(),
        params: function
            .params
            .iter()
            .map(|param| waluau_ast::Param {
                name: param.name.clone(),
                symbol_id: param.symbol_id,
                ty: erase_type_opaque_types(&param.ty),
            })
            .collect(),
        vararg: function.vararg.clone(),
        return_type: function
            .return_type
            .as_ref()
            .map(erase_type_opaque_types),
        body: function.body.iter().map(erase_stmt_opaque_types).collect(),
        file_path: function.file_path.clone(),
        span: function.span,
    }
}

fn erase_stmt_opaque_types(stmt: &Stmt) -> Stmt {
    match stmt {
        Stmt::Let {
            name,
            symbol_id,
            rebindability,
            ty,
            value,
        } => Stmt::Let {
            name: name.clone(),
            symbol_id: *symbol_id,
            rebindability: *rebindability,
            ty: ty.as_ref().map(erase_type_opaque_types),
            value: erase_expr_opaque_types(value),
        },
        Stmt::Assign { op, name, symbol_id, value } => Stmt::Assign {
            op: *op,
            name: name.clone(),
            symbol_id: *symbol_id,
            value: erase_expr_opaque_types(value),
        },
        Stmt::IndexAssign {
            op,
            base,
            index,
            value,
        } => Stmt::IndexAssign {
            op: *op,
            base: Box::new(erase_expr_opaque_types(base)),
            index: Box::new(erase_expr_opaque_types(index)),
            value: erase_expr_opaque_types(value),
        },
        Stmt::FieldAssign {
            op,
            base,
            name,
            resolved_name,
            value,
        } => Stmt::FieldAssign {
            op: *op,
            base: Box::new(erase_expr_opaque_types(base)),
            name: name.clone(),
            resolved_name: resolved_name.clone(),
            value: erase_expr_opaque_types(value),
        },
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => Stmt::If {
            condition: erase_expr_opaque_types(condition),
            then_body: then_body.iter().map(erase_stmt_opaque_types).collect(),
            else_body: else_body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::IfCast {
            target_name,
            target_ty,
            binding,
            binding_symbol_id,
            value,
            then_body,
            else_body,
        } => Stmt::IfCast {
            target_name: target_name.clone(),
            target_ty: erase_type_opaque_types(target_ty),
            binding: binding.clone(),
            binding_symbol_id: *binding_symbol_id,
            value: erase_expr_opaque_types(value),
            then_body: then_body.iter().map(erase_stmt_opaque_types).collect(),
            else_body: else_body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::Match {
            value,
            enum_ty,
            arms,
        } => Stmt::Match {
            value: erase_expr_opaque_types(value),
            enum_ty: erase_type_opaque_types(enum_ty),
            arms: arms
                .iter()
                .map(|arm| waluau_ast::EnumMatchArm {
                    variant: arm.variant.clone(),
                    ordinal: arm.ordinal,
                    body: arm.body.iter().map(erase_stmt_opaque_types).collect(),
                })
                .collect(),
        },
        Stmt::While { condition, body } => Stmt::While {
            condition: erase_expr_opaque_types(condition),
            body: body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::Repeat { body, condition } => Stmt::Repeat {
            body: body.iter().map(erase_stmt_opaque_types).collect(),
            condition: erase_expr_opaque_types(condition),
        },
        Stmt::NumericFor {
            name,
            symbol_id,
            start,
            stop,
            step,
            body,
        } => Stmt::NumericFor {
            name: name.clone(),
            symbol_id: *symbol_id,
            start: erase_expr_opaque_types(start),
            stop: erase_expr_opaque_types(stop),
            step: step.as_ref().map(erase_expr_opaque_types),
            body: body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::ForIn {
            names,
            symbol_ids,
            iterators,
            body,
        } => Stmt::ForIn {
            names: names.clone(),
            symbol_ids: symbol_ids.clone(),
            iterators: iterators.iter().map(erase_expr_opaque_types).collect(),
            body: body.iter().map(erase_stmt_opaque_types).collect(),
        },
        Stmt::Break => Stmt::Break,
        Stmt::Continue => Stmt::Continue,
        Stmt::Return(value) => Stmt::Return(erase_expr_opaque_types(value)),
        Stmt::ReturnMulti(values) => {
            Stmt::ReturnMulti(values.iter().map(erase_expr_opaque_types).collect())
        }
        Stmt::LetMulti { bindings, values } => Stmt::LetMulti {
            bindings: bindings
                .iter()
                .map(|binding| waluau_ast::Binding {
                    name: binding.name.clone(),
                    symbol_id: binding.symbol_id,
                    rebindability: binding.rebindability,
                    ty: binding.ty.as_ref().map(erase_type_opaque_types),
                })
                .collect(),
            values: values.iter().map(erase_expr_opaque_types).collect(),
        },
        Stmt::AssignMulti { targets, symbol_ids, values } => Stmt::AssignMulti {
            targets: targets.clone(),
            symbol_ids: symbol_ids.clone(),
            values: values.iter().map(erase_expr_opaque_types).collect(),
        },
        Stmt::Expr(expr) => Stmt::Expr(erase_expr_opaque_types(expr)),
    }
}

fn erase_expr_opaque_types(expr: &Expr) -> Expr {
    match expr {
        Expr::Number(..)
        | Expr::Bool(..)
        | Expr::Nil(..)
        | Expr::String(..)
        | Expr::Bytes(..)
        | Expr::Vararg(..)
        | Expr::Name(..)
        | Expr::Require(..) => expr.clone(),
        Expr::IsVariant { expr, tag, span } => Expr::IsVariant {
            expr: Box::new(erase_expr_opaque_types(expr)),
            tag: tag.clone(),
            span: *span,
        },
        Expr::Unary {
            op,
            expr,
            resolved_name,
            span,
        } => Expr::Unary {
            op: *op,
            expr: Box::new(erase_expr_opaque_types(expr)),
            resolved_name: resolved_name.clone(),
            span: *span,
        },
        Expr::Cast { expr, ty, span } => Expr::Cast {
            expr: Box::new(erase_expr_opaque_types(expr)),
            ty: erase_type_opaque_types(ty),
            span: *span,
        },
        Expr::Binary {
            op,
            left,
            right,
            resolved_name,
            span,
        } => Expr::Binary {
            op: *op,
            left: Box::new(erase_expr_opaque_types(left)),
            right: Box::new(erase_expr_opaque_types(right)),
            resolved_name: resolved_name.clone(),
            span: *span,
        },
        Expr::If {
            condition,
            then_expr,
            else_expr,
            span,
        } => Expr::If {
            condition: Box::new(erase_expr_opaque_types(condition)),
            then_expr: Box::new(erase_expr_opaque_types(then_expr)),
            else_expr: Box::new(erase_expr_opaque_types(else_expr)),
            span: *span,
        },
        Expr::Call {
            callee,
            type_args,
            args,
            span,
            method_call_origin,
        } => Expr::Call {
            callee: Box::new(erase_expr_opaque_types(callee)),
            type_args: type_args.iter().map(erase_type_opaque_types).collect(),
            args: args.iter().map(erase_expr_opaque_types).collect(),
            span: *span,
            method_call_origin: method_call_origin.clone(),
        },
        Expr::MethodCall {
            receiver,
            name,
            resolved_name,
            args,
            span,
            type_args,
        } => Expr::MethodCall {
            receiver: Box::new(erase_expr_opaque_types(receiver)),
            name: name.clone(),
            resolved_name: resolved_name.clone(),
            args: args.iter().map(erase_expr_opaque_types).collect(),
            span: *span,
            type_args: type_args.clone(),
        },
        Expr::Function(function) => Expr::Function(waluau_ast::FunctionExpr {
            name: function.name.clone(),
            declaration_class: function.declaration_class,
            symbol_id: function.symbol_id,
            implicit_self: function.implicit_self.clone(),
            type_params: function.type_params.clone(),
            params: function
                .params
                .iter()
                .map(|param| waluau_ast::Param {
                    name: param.name.clone(),
                    symbol_id: param.symbol_id,
                    ty: erase_type_opaque_types(&param.ty),
                })
                .collect(),
            vararg: function.vararg.clone(),
            return_type: function.return_type.as_ref().map(erase_type_opaque_types),
            body: function.body.iter().map(erase_stmt_opaque_types).collect(),
            file_path: function.file_path.clone(),
            span: function.span,
        }),
        Expr::ArrayLiteral { elements, span } => Expr::ArrayLiteral {
            elements: elements.iter().map(erase_expr_opaque_types).collect(),
            span: *span,
        },
        Expr::TableLiteral { fields, span } => Expr::TableLiteral {
            fields: fields
                .iter()
                .map(|field| waluau_ast::TableField {
                    name: field.name.clone(),
                    value: erase_expr_opaque_types(&field.value),
                })
                .collect(),
            span: *span,
        },
        Expr::Field {
            base,
            name,
            resolved_name,
            span,
        } => Expr::Field {
            base: Box::new(erase_expr_opaque_types(base)),
            name: name.clone(),
            resolved_name: resolved_name.clone(),
            span: *span,
        },
        Expr::Index { base, index, span } => Expr::Index {
            base: Box::new(erase_expr_opaque_types(base)),
            index: Box::new(erase_expr_opaque_types(index)),
            span: *span,
        },
    }
}

fn erase_type_opaque_types(ty: &Type) -> Type {
    erase_type_opaque_types_at(ty, false)
}

/// Whether erasure would change `ty` at all. Most types in a large program
/// are already in runtime form; returning the same Arc-shared value for
/// those keeps erasure allocation-free and preserves pointer identity for
/// the codegen registries.
fn type_needs_erasure(ty: &Type) -> bool {
    match ty {
        Type::Opaque { .. } | Type::ExternSubtype(_) | Type::StringLiteralUnion(_) => true,
        Type::Nullable(inner) | Type::Array(inner) | Type::Variadic(inner) => {
            type_needs_erasure(inner)
        }
        Type::Multi(types) => types.iter().any(type_needs_erasure),
        Type::Function {
            params,
            return_type,
            has_self,
        } => *has_self || params.iter().any(type_needs_erasure) || type_needs_erasure(return_type),
        Type::Record(fields) => fields.values().any(type_needs_erasure),
        Type::TaggedVariant(variant) => type_needs_erasure(&variant.payload),
        Type::TaggedUnion(variants) => variants
            .iter()
            .any(|variant| type_needs_erasure(&variant.payload)),
        _ => false,
    }
}

fn erase_type_opaque_types_at(ty: &Type, nested: bool) -> Type {
    if !type_needs_erasure(ty) {
        return ty.clone();
    }
    match ty {
        Type::Opaque { name, ty, .. } if nested && type_contains_opaque_name(ty, name) => {
            Type::Unknown
        }
        Type::Opaque { ty, .. } => erase_type_opaque_types_at(ty, nested),
        Type::ExternSubtype(_) => Type::Extern,
        // A string literal union is a type-checking construct; at runtime
        // its value is the member string.
        Type::StringLiteralUnion(_) => Type::String,
        Type::Nullable(inner) => Type::Nullable(Arc::new(erase_type_opaque_types_at(inner, true))),
        Type::Array(inner) => Type::Array(Arc::new(erase_type_opaque_types_at(inner, true))),
        Type::Variadic(inner) => Type::Variadic(Arc::new(erase_type_opaque_types_at(inner, true))),
        Type::Multi(types) => Type::Multi(
            types
                .iter()
                .map(|ty| erase_type_opaque_types_at(ty, nested))
                .collect(),
        ),
        Type::Function {
            params,
            return_type,
            // `self` is a checker-only receiver contract: at runtime a method
            // slot holds a bound closure whose callable signature is exactly
            // the self-less parameter list. Erasing the marker here keeps
            // struct keying, CallValue wrapper signatures, and the generic
            // indirect call path consistent — IR and codegen never see
            // `has_self`.
            has_self: _,
        } => Type::Function {
            params: params
                .iter()
                .map(|ty| erase_type_opaque_types_at(ty, false))
                .collect(),
            return_type: Arc::new(erase_type_opaque_types_at(return_type, false)),
            has_self: false,
        },
        Type::Record(fields) => Type::record(
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), erase_type_opaque_types_at(ty, true)))
                .collect(),
        ),
        // A payload is stored behind the `value` field of the canonical
        // `{ tag, value }` record, so it erases exactly like a record field
        // does. Skipping it left aliases and literal unions alive inside
        // variants long after every other type position had shed them.
        Type::TaggedVariant(variant) => Type::TaggedVariant(TaggedVariant {
            tag: variant.tag.clone(),
            payload: Arc::new(erase_type_opaque_types_at(&variant.payload, true)),
        }),
        Type::TaggedUnion(variants) => Type::TaggedUnion(
            variants
                .iter()
                .map(|variant| TaggedVariant {
                    tag: variant.tag.clone(),
                    payload: Arc::new(erase_type_opaque_types_at(&variant.payload, true)),
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn type_contains_opaque_name(ty: &Type, target: &str) -> bool {
    match ty {
        Type::Opaque { name, .. } if name == target => true,
        Type::Opaque { ty, .. } => type_contains_opaque_name(ty, target),
        Type::ExternSubtype(ty)
        | Type::Nullable(ty)
        | Type::Array(ty)
        | Type::Variadic(ty) => type_contains_opaque_name(ty, target),
        Type::Multi(types) => types
            .iter()
            .any(|ty| type_contains_opaque_name(ty, target)),
        Type::Function {
            params,
            return_type,
            ..
        } => {
            params
                .iter()
                .any(|ty| type_contains_opaque_name(ty, target))
                || type_contains_opaque_name(return_type, target)
        }
        Type::Record(fields) => fields
            .values()
            .any(|ty| type_contains_opaque_name(ty, target)),
        Type::TaggedVariant(variant) => type_contains_opaque_name(&variant.payload, target),
        Type::TaggedUnion(variants) => variants
            .iter()
            .any(|variant| type_contains_opaque_name(&variant.payload, target)),
        _ => false,
    }
}

/// `TaggedUnion`/`TaggedVariant` source-level types are represented at the IR
/// level by the canonical `{ tag: i32, value: unknown }` record. Use this
/// whenever a source-level type ends up annotating an IR instruction (array
/// cells, casts, etc.) so the annotation matches the value's actual runtime
/// representation that `verify` checks against.
fn to_runtime_type(ty: &Type) -> Type {
    ty.runtime_representation()
}

fn function_type(params: Vec<Type>, return_type: Type) -> Type {
    Type::Function {
        params,
        return_type: Arc::new(return_type),
        has_self: false,
    }
}

/// Module-wide lookup tables shared by every function lowering. Built once per
/// module and passed by shared reference, so the parallel lowering workers can
/// borrow one context across `std::thread::scope`.
pub(crate) struct ModuleLoweringContext<'a> {
    pub(crate) signatures: &'a HashMap<SymbolId, (Vec<Type>, Type)>,
    pub(crate) host_import_signatures: &'a HashMap<SymbolId, (Vec<Type>, Type)>,
    pub(crate) host_import_names: &'a HashMap<String, SymbolId>,
    pub(crate) field_call_signatures: &'a HashMap<String, (Vec<Type>, Type)>,
    /// Declared namespace constants (`declare const math.pi: f64 = ...`),
    /// keyed by qualified name; field reads fold to the literal.
    pub(crate) declared_constants: &'a HashMap<String, (Type, NumberLiteral)>,
    pub(crate) globals: &'a HashMap<SymbolId, (usize, Type)>,
    pub(crate) sources: &'a BTreeMap<String, String>,
    pub(crate) source_file_ids: &'a HashMap<String, SourceFileId>,
    /// Stable discriminant IDs for tagged-union variant names, shared across the
    /// whole module so constructors and checks in different functions agree.
    pub(crate) tag_ids: &'a BTreeMap<String, i32>,
}

pub(crate) fn build_function(
    function: &AstFunction,
    cx: &ModuleLoweringContext<'_>,
) -> Result<Vec<Function>, Diagnostic> {
    let source_file = cx
        .source_file_ids
        .get(&function.file_path)
        .copied()
        .expect("lowered function source must be interned");
    let return_type = function.return_type.clone().ok_or_else(|| {
        inference_diagnostic(
            "inference/unsupported",
            DiagnosticCategory::Unsupported,
            format!(
                "function '{}' must have a concrete return type before IR lowering",
                function.name
            ),
            "ensure type inference runs before IR lowering or add an explicit return type",
        )
    })?;
    let mut out = Function {
        name: function.name.to_string(),
        params: {
            let mut params = function
                .params
                .iter()
                .map(|param| (param.name.clone(), param.ty.clone()))
                .collect::<Vec<_>>();
            if function.vararg.is_some() {
                params.push((
                    "...".to_string(),
                    Type::Variadic(Arc::new(Type::Unknown)),
                ));
            }
            params
        },
        return_type,
        entry: BlockId(0),
        blocks: BTreeMap::new(),
        next_value: 0,
        capture_count: 0,
        value_symbols: BTreeMap::new(),
        symbol_id: function.symbol_id,
        source_map: FunctionSourceMap {
            definition: function.span.map_or(SourceOrigin::Synthetic, |span| {
                SourceOrigin::Authored(SourceLocation {
                    file: source_file,
                    span,
                })
            }),
            instruction_locations: BTreeMap::new(),
            terminator_locations: BTreeMap::new(),
        },
    };

    out.blocks.insert(
        out.entry,
        BasicBlock {
            id: out.entry,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable { span: None },
        },
    );

    let mut env = HashMap::new();
    let mut type_env = cx
        .globals
        .iter()
        .map(|(symbol_id, (_, ty))| (*symbol_id, ty.clone()))
        .collect::<HashMap<_, _>>();
    let entry = out.entry;
    let captured_symbols: HashSet<SymbolId> = collect_nested_function_capture_names(function);

    for (index, param) in function.params.iter().enumerate() {
        let symbol_id = param.symbol_id.expect("param has resolved symbol_id");
        let value = out.next_value();
        block_mut(&mut out, entry)
            .instructions
            .push((value, Instruction::Param(index)));
        if captured_symbols.contains(&symbol_id) {
            let cell = out.next_value();
            block_mut(&mut out, entry).instructions.push((
                cell,
                Instruction::ArrayNew {
                    element_ty: to_runtime_type(&param.ty),
                    elements: vec![value],
                },
            ));
            env.insert(symbol_id, cell);
            out.value_symbols.insert(cell, symbol_id);
        } else {
            env.insert(symbol_id, value);
        }
        out.value_symbols.insert(value, symbol_id);
        type_env.insert(symbol_id, param.ty.clone());
    }
    let vararg_value = if function.vararg.is_some() {
        let value = out.next_value();
        block_mut(&mut out, entry)
            .instructions
            .push((value, Instruction::Param(function.params.len())));
        Some(value)
    } else {
        None
    };
    let vararg_element = function.vararg.clone();

    let symbol_storage_types = type_env.clone();
    let mut builder = Builder {
        function: out,
        current_block: BlockId(0),
        next_block: 1,
        cx,
        lifted_functions: Vec::new(),
        lambda_counter: 0,
        loop_stack: Vec::new(),
        cell_names: captured_symbols,
        file_path: function.file_path.clone(),
        source_file: function.span.map(|_| source_file),
        current_span: None,
        vararg_value,
        vararg_element,
        discriminants: HashMap::new(),
        symbol_storage_types,
    };
    let mut scope = Scope {
        env,
        types: type_env,
    };
    for stmt in &function.body {
        if builder.current_block == DEAD_BLOCK {
            break;
        }
        builder.lower_stmt(stmt, &mut scope)?;
    }
    if builder.current_block != DEAD_BLOCK && builder.function.return_type == Type::Unit {
        let value = builder.emit(Instruction::Unit);
        builder.set_terminator(builder.current_block, Terminator::Return(value));
        builder.current_block = DEAD_BLOCK;
    }
    // A nullable-returning function that falls off the end returns nil, as in
    // Lua (functions without an explicit return produce nil).
    if builder.current_block != DEAD_BLOCK
        && matches!(builder.function.return_type, Type::Nullable(_))
    {
        let return_type = builder.function.return_type.clone();
        let value = builder.emit(Instruction::Null { ty: return_type });
        builder.set_terminator(builder.current_block, Terminator::Return(value));
        builder.current_block = DEAD_BLOCK;
    }
    let mut functions = vec![builder.function];
    functions.extend(builder.lifted_functions);
    Ok(functions)
}

const DEAD_BLOCK: BlockId = BlockId(usize::MAX);

type HostImportValue = (String, SymbolId, Vec<Type>, Type);

/// Statements do not yet carry their own delimiter-to-delimiter spans, so use
/// the primary authored expression as their stable line-program location.
/// Break/continue have no expression span and therefore remain synthetic until
/// the AST grows statement spans.
fn primary_stmt_span(stmt: &Stmt) -> Option<waluau_ast::Span> {
    match stmt {
        Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::Return(value)
        | Stmt::Expr(value) => value.span(),
        Stmt::IndexAssign { base, .. } | Stmt::FieldAssign { base, .. } => base.span(),
        Stmt::If { condition, .. } | Stmt::While { condition, .. } => condition.span(),
        Stmt::IfCast { value, .. } | Stmt::Match { value, .. } => value.span(),
        Stmt::Repeat { condition, .. } => condition.span(),
        Stmt::NumericFor { start, .. } => start.span(),
        Stmt::ForIn { iterators, .. } => iterators.first().and_then(Expr::span),
        Stmt::ReturnMulti(values)
        | Stmt::LetMulti { values, .. }
        | Stmt::AssignMulti { values, .. } => values.first().and_then(Expr::span),
        Stmt::Break | Stmt::Continue => None,
    }
}

/// Branch-local type narrowing derived from an `if` condition: the narrowed
/// types to apply on entry to each arm (see `narrowed_type_scopes`).
struct BranchNarrowing {
    then_types: HashMap<SymbolId, Type>,
    else_types: HashMap<SymbolId, Type>,
}

/// The lexical scope threaded through statement and expression lowering: the
/// current SSA value for each named symbol together with its flow-narrowed
/// source-level type. The two maps always travel together; forking clones both
/// for a branch- or loop-local scope whose rebindings must not leak out.
struct Scope {
    /// Current SSA value (or mutable-capture cell) for each symbol.
    env: HashMap<SymbolId, ValueId>,
    /// Source-level type for each symbol, including branch-local narrowing.
    types: HashMap<SymbolId, Type>,
}

impl Scope {
    fn fork(&self) -> Scope {
        Scope {
            env: self.env.clone(),
            types: self.types.clone(),
        }
    }
}

struct Builder<'a> {
    function: Function,
    current_block: BlockId,
    next_block: usize,
    cx: &'a ModuleLoweringContext<'a>,
    lifted_functions: Vec<Function>,
    lambda_counter: usize,
    loop_stack: Vec<LoopContext>,
    /// SymbolIds that are represented as 1-element array "cells" to support mutable capture.
    cell_names: HashSet<SymbolId>,
    file_path: String,
    source_file: Option<SourceFileId>,
    /// Current authored expression/statement. `None` means emitted scaffolding
    /// is compiler-synthesized and must remain unmapped.
    current_span: Option<waluau_ast::Span>,
    vararg_value: Option<ValueId>,
    /// The annotated element type of the enclosing function's `...` (from
    /// `function f(...: T)`), when one was written. Static only: the pack
    /// value itself always stays an `Array(unknown)` at runtime, and uses of
    /// `...` unbox through expected-driven casts.
    vararg_element: Option<Type>,
    /// For `local ok, v = pcall(...)`: maps the `ok` symbol to its payload
    /// symbol and the payload types on the success/failure paths, so branching
    /// on `ok` (or `assert(ok)`) narrows `v` and unboxes it from its anyref slot.
    discriminants: HashMap<SymbolId, DiscriminantLink>,
    /// Runtime storage types for named values. Flow narrowing may replace a
    /// record field's source-level type in the branch-local type environment,
    /// but it does not change the shape of the record already in storage.
    symbol_storage_types: HashMap<SymbolId, Type>,
}

#[derive(Clone)]
struct DiscriminantLink {
    payload: SymbolId,
    when_true: Type,
    when_false: Type,
}

/// Whether a pcall payload type can be soundly materialized from the `unknown`
/// storage slot by a runtime unbox cast when narrowing applies. Mirrors
/// `is_narrowable_pcall_payload` in waluau-hir.
fn is_narrowable_pcall_payload(ty: &Type) -> bool {
    !matches!(ty, Type::Unit | Type::Unknown | Type::Nil | Type::Multi(_))
}

#[derive(Clone)]
struct LoopContext {
    header: BlockId,
    continue_target: BlockId,
    break_target: BlockId,
    phis: HashMap<SymbolId, ValueId>,
    /// Implicit loop-carried header phis that are not tied to a source symbol
    /// (the numeric-for index/stop bound, the array for-in index/length). These
    /// are advanced when control falls off the bottom of the body, but a
    /// `continue` jumps straight back to the header, so each continue site must
    /// replay these updates too or the header phis end up missing an incoming
    /// edge.
    carries: Vec<LoopCarry>,
    /// Phis in the loop exit block, one per mutated variable. The exit block is
    /// reached both from the loop's normal exit edge (with the header phi /
    /// end-of-body value) and from every `break` site (with the values live at
    /// the break). Code after the loop must read these exit phis; reading the
    /// header phis would resurrect start-of-iteration values on break paths.
    exit_phis: HashMap<SymbolId, ValueId>,
}

#[derive(Clone)]
struct LoopCarry {
    /// The header phi that needs an incoming value for every `continue` edge.
    phi: ValueId,
    update: CarryUpdate,
}

#[derive(Clone)]
enum CarryUpdate {
    /// A loop-invariant carried value (the numeric-for stop bound, the array
    /// length): the same SSA value flows along every edge into the header.
    Invariant(ValueId),
    /// A per-iteration increment recomputed as `base + step` in the continue
    /// block (the numeric-for / for-in loop index).
    Increment {
        base: ValueId,
        step: ValueId,
        ty: Type,
    },
}

/// How `lower_indexed_for_in` binds the loop variables for each element.
enum IndexedLoopBindings {
    /// `for v in arr` / `for i, v in arr`: the element, or the 1-based index
    /// plus the element.
    Array,
    /// `for i[, v] in next, arr`: the 1-based index plus, when bound, the
    /// element — `next` yields keys first, so one variable binds the index.
    ArrayNext,
    /// `for name in pairs(rec)` / `for name, value in pairs(rec)`: the
    /// field-name element plus, when a value variable is bound, the field
    /// value read from a parallel array at the same index.
    RecordPairs { values: Option<(ValueId, Type)> },
}

/// Where a generic-for protocol loop gets its iterator, state, and control:
/// an explicit `f, s[, c0]` expression list, or one expression whose type is
/// an `(iterator, state[, control])` multi-value (a hand-written `pairs`
/// factory call).
enum ProtocolSource<'a> {
    ExprList {
        function: &'a Expr,
        state: &'a Expr,
        control: Option<&'a Expr>,
    },
    MultiValue { expr: &'a Expr, slots: Vec<Type> },
}

/// Recognizes `string.gmatch(s, p)` / `s:gmatch(p)` in for-in iterator
/// position and returns the (haystack, pattern) argument expressions.
fn gmatch_iterator_args(iterator: &Expr) -> Option<(&Expr, &Expr)> {
    match iterator {
        Expr::Call { callee, args, .. }
            if builtin_name(callee).as_deref() == Some(STRING_GMATCH) && args.len() == 2 =>
        {
            Some((&args[0], &args[1]))
        }
        Expr::MethodCall {
            receiver,
            name,
            args,
            ..
        } if name == "gmatch" && args.len() == 1 => Some((receiver.as_ref(), &args[0])),
        _ => None,
    }
}

/// Lua multi-value adjustment for a type in a single-value context.
fn first_of_multi(ty: Type) -> Type {
    match ty {
        Type::Multi(mut parts) if !parts.is_empty() => parts.remove(0),
        other => other,
    }
}

/// Whether a `bit32` argument that missed the direct `u32`/`i32` path is still
/// a number Luau would accept. Wider numeric types and `unknown` carry values
/// outside the 32-bit range, so they route through the modulo-2^32 conversion
/// rather than failing the call.
fn bit32_dynamic_operand(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Unknown
            | Type::Numeric(
                NumericType::F64 | NumericType::F32 | NumericType::I64 | NumericType::U64
            )
    )
}

/// Whether an operand is an integer literal, optionally negated, that already
/// lowers to the exact 32-bit constant the intrinsic wants. A literal outside
/// that range (or written in float form) takes the conversion path instead,
/// where Luau's modulo-2^32 rule applies.
fn bit32_direct_literal(expr: &Expr, expected: &Type) -> bool {
    fn literal_value(expr: &Expr, negated: bool) -> Option<i128> {
        match expr {
            Expr::Number(number, _) => number
                .int_value()
                .map(|value| if negated { -value } else { value }),
            Expr::Unary {
                op: waluau_ast::UnaryOp::Neg,
                expr,
                ..
            } => literal_value(expr, !negated),
            _ => None,
        }
    }
    let Some(value) = literal_value(expr, false) else {
        return false;
    };
    // A negative literal is written as the two's-complement pattern in either
    // position, so both share the same lower bound.
    let upper = if expected == &Type::Numeric(NumericType::U32) {
        u32::MAX as i128
    } else {
        i32::MAX as i128
    };
    (i32::MIN as i128..=upper).contains(&value)
}

/// The diagnostic for a `bit32` argument that is neither a 32-bit value nor a
/// number Luau would convert, preferring the inner inference error over the
/// generic shape mismatch.
fn bit32_operand_mismatch(
    name: &str,
    index: usize,
    expected: &Type,
    typed: Result<Type, Diagnostic>,
) -> Diagnostic {
    match typed {
        Err(error) => error,
        Ok(actual) => Diagnostic::new(format!(
            "{name} expects {expected} argument #{}, got {actual}",
            index + 1
        )),
    }
}

/// One result slot of a `pm_find`/`pm_match` call.
#[derive(Clone, Copy)]
enum PmSlot {
    /// 1-based match start; nil when there is no match.
    Start,
    /// 1-based inclusive match end; 0 when there is no match.
    End,
    /// A capture (host index 0 is the whole match).
    Capture {
        kind: waluau_ast::LuaCaptureKind,
        index: i32,
        nullable: bool,
    },
}

impl PmSlot {
    fn capture(kind: waluau_ast::LuaCaptureKind, index: i32, nullable: bool) -> Self {
        PmSlot::Capture {
            kind,
            index,
            nullable,
        }
    }

    fn value_type(&self) -> Type {
        let i32_ty = Type::Numeric(NumericType::I32);
        match self {
            PmSlot::Start => Type::Nullable(Arc::new(i32_ty)),
            PmSlot::End => i32_ty,
            PmSlot::Capture {
                kind, nullable, ..
            } => {
                let inner = kind.value_type();
                if *nullable {
                    Type::Nullable(Arc::new(inner))
                } else {
                    inner
                }
            }
        }
    }
}

fn builtin_name(callee: &Expr) -> Option<String> {
    match callee {
        Expr::Name(name, _, _) => Some(name.clone()),
        Expr::Field { base, name, .. } => match base.as_ref() {
            Expr::Name(namespace, _, _) => Some(format!("{namespace}.{name}")),
            _ => None,
        },
        _ => None,
    }
}

fn json_tag_payload_supported(ty: &Type) -> bool {
    match ty {
        Type::Opaque { ty, .. } => json_tag_payload_supported(ty),
        Type::Nullable(ty) => json_tag_payload_supported(ty),
        // Tagged payloads are stored in `unknown` (anyref). Strings and bytes
        // are externref-shaped and cannot be boxed into that slot.
        Type::String | Type::Bytes | Type::Buffer | Type::StringLiteralUnion(_) => false,
        _ => json_supported_type(ty),
    }
}

fn json_supported_type(ty: &Type) -> bool {
    match ty {
        Type::Numeric(_)
        | Type::Bool
        | Type::String
        | Type::Bytes
        | Type::Unit
        | Type::StringLiteralUnion(_)
        | Type::TypeParam(_) => true,
        Type::Opaque { ty, .. } => json_supported_type(ty),
        Type::Nullable(ty)
        | Type::Array(ty) => json_supported_type(ty),
        Type::TypedArray(_) => true,
        Type::Buffer => false,
        Type::Record(fields) => fields.values().all(json_supported_type),
        Type::TaggedVariant(variant) => json_tag_payload_supported(&variant.payload),
        Type::TaggedUnion(variants) => variants
            .iter()
            .all(|variant| json_tag_payload_supported(&variant.payload)),
        Type::Nil
        | Type::Extern
        | Type::ExternSubtype(_)
        | Type::Named { .. }
        | Type::Variadic(_)
        | Type::Multi(_)
        | Type::Function { .. }
        | Type::Thread
        | Type::Unknown => false,
    }
}

fn json_unpack_value_type(target: &Type) -> Type {
    if target.accepts_nil() {
        target.clone()
    } else {
        Type::Nullable(Arc::new(target.clone()))
    }
}

fn json_pack_numeric_host(ty: NumericType) -> &'static str {
    match ty {
        NumericType::I32 => JSON_PACK_I32_HOST,
        NumericType::U32 => JSON_PACK_U32_HOST,
        NumericType::I64 => JSON_PACK_I64_HOST,
        NumericType::U64 => JSON_PACK_U64_HOST,
        NumericType::F32 => JSON_PACK_F32_HOST,
        NumericType::F64 => JSON_PACK_F64_HOST,
    }
}

fn json_unpack_numeric_host(ty: NumericType) -> &'static str {
    match ty {
        NumericType::I32 => JSON_UNPACK_I32_HOST,
        NumericType::U32 => JSON_UNPACK_U32_HOST,
        NumericType::I64 => JSON_UNPACK_I64_HOST,
        NumericType::U64 => JSON_UNPACK_U64_HOST,
        NumericType::F32 => JSON_UNPACK_F32_HOST,
        NumericType::F64 => JSON_UNPACK_F64_HOST,
    }
}

fn json_quote_schema_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn json_numeric_schema_name(ty: NumericType) -> &'static str {
    match ty {
        NumericType::I32 => "i32",
        NumericType::U32 => "u32",
        NumericType::I64 => "i64",
        NumericType::U64 => "u64",
        NumericType::F32 => "f32",
        NumericType::F64 => "f64",
    }
}

fn json_schema(ty: &Type) -> Result<String, Diagnostic> {
    let schema = match ty {
        Type::Opaque { ty, .. } => return json_schema(ty),
        Type::Numeric(numeric) => format!("{{\"t\":\"{}\"}}", json_numeric_schema_name(*numeric)),
        Type::Bool => "{\"t\":\"bool\"}".to_string(),
        Type::String => "{\"t\":\"string\"}".to_string(),
        Type::StringLiteralUnion(members) => {
            let members = members
                .iter()
                .map(|member| json_quote_schema_string(member))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{\"t\":\"string-union\",\"m\":[{members}]}}")
        }
        Type::Bytes => "{\"t\":\"bytes\"}".to_string(),
        Type::Unit => "{\"t\":\"unit\"}".to_string(),
        Type::Nullable(inner) => {
            format!("{{\"t\":\"nullable\",\"v\":{}}}", json_schema(inner)?)
        }
        Type::Array(element) => {
            format!("{{\"t\":\"array\",\"v\":{}}}", json_schema(element)?)
        }
        Type::TypedArray(kind) => format!(
            "{{\"t\":\"array\",\"v\":{{\"t\":\"{}\"}}}}",
            json_numeric_schema_name(kind.element_numeric_type())
        ),
        Type::Record(fields) => {
            let fields = fields
                .iter()
                .map(|(name, ty)| {
                    Ok(format!(
                        "[{},{}]",
                        json_quote_schema_string(name),
                        json_schema(ty)?
                    ))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?
                .join(",");
            format!("{{\"t\":\"record\",\"f\":[{fields}]}}")
        }
        Type::TaggedVariant(variant) => {
            let payload = json_schema(&variant.payload)?;
            format!(
                "{{\"t\":\"tagged\",\"v\":[[{},{}]]}}",
                json_quote_schema_string(&variant.tag),
                payload
            )
        }
        Type::TaggedUnion(variants) => {
            let variants = variants
                .iter()
                .map(|variant| {
                    Ok(format!(
                        "[{},{}]",
                        json_quote_schema_string(&variant.tag),
                        json_schema(&variant.payload)?
                    ))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?
                .join(",");
            format!("{{\"t\":\"tagged\",\"v\":[{variants}]}}")
        }
        other => {
            return Err(Diagnostic::new(format!(
                "json.unpack does not support values of type {other}"
            )));
        }
    };
    Ok(schema)
}

fn string_byte_static_count(args: &[Expr], expected: Option<&Type>) -> Result<usize, Diagnostic> {
    if let Some(Type::Multi(types)) = expected {
        return Ok(types.len());
    }
    let indices = string_byte_static_indices(args, expected)?;
    Ok(indices.len())
}

fn string_byte_requires_numeric_value(expected: Option<&Type>) -> bool {
    match expected {
        Some(Type::Numeric(_)) => true,
        Some(Type::Multi(types)) if types.len() == 1 => {
            matches!(types.first(), Some(Type::Numeric(_)))
        }
        _ => false,
    }
}

fn string_byte_scalar_known_in_range(args: &[Expr]) -> bool {
    let Some(len) = args.first().and_then(expr_string_len) else {
        return false;
    };
    let index = args.get(1).and_then(expr_i32_literal).unwrap_or(1);
    let index = if index < 0 {
        len.saturating_add(index).saturating_add(1)
    } else {
        index
    };
    index >= 1 && index <= len
}

fn string_byte_static_indices(
    args: &[Expr],
    expected: Option<&Type>,
) -> Result<Vec<i32>, Diagnostic> {
    let start = args.get(1).and_then(expr_i32_literal).unwrap_or(1);
    let Some(end) = args.get(2).and_then(expr_i32_literal) else {
        return Err(Diagnostic::new(
            "string.byte range form requires literal indices or an expected multi-value arity",
        ));
    };
    let count_from_expected = expected.and_then(|ty| match ty {
        Type::Multi(types) => Some(types.len()),
        _ => None,
    });
    let (start, end) = if let Some(len) = args.first().and_then(expr_string_len) {
        clip_lua_byte_range(start, end, len)
    } else if start >= 1 && end >= 0 {
        (start, end)
    } else if let Some(count) = count_from_expected {
        return Ok((0..count).map(|offset| start + offset as i32).collect());
    } else {
        return Err(Diagnostic::new(
            "string.byte range with negative indices requires a literal string",
        ));
    };
    if end < start {
        Ok(Vec::new())
    } else {
        Ok((start..=end).collect())
    }
}

// expr_i32_literal, table_unpack_static_indices, and expr_static_array_len
// live at the crate root (lib.rs) because the monomorphizer needs them too.

fn expr_string_len(expr: &Expr) -> Option<i32> {
    let Expr::String(value, _) = expr else {
        return None;
    };
    i32::try_from(value.chars().count()).ok()
}

/// `Float32Array.create` (and the other typed-array kinds' `create`).
fn typed_array_create_kind(name: &str) -> Option<TypedArrayKind> {
    let (type_name, member) = name.split_once('.')?;
    if member != "create" {
        return None;
    }
    TypedArrayKind::from_type_name(type_name)
}

fn buffer_scalar_kind(member: &str) -> Option<TypedArrayKind> {
    match member {
        "i8" => Some(TypedArrayKind::I8),
        "u8" => Some(TypedArrayKind::U8),
        "i16" => Some(TypedArrayKind::I16),
        "u16" => Some(TypedArrayKind::U16),
        "i32" => Some(TypedArrayKind::I32),
        "u32" => Some(TypedArrayKind::U32),
        "f32" => Some(TypedArrayKind::F32),
        "f64" => Some(TypedArrayKind::F64),
        _ => None,
    }
}

/// When every element of a typed-array literal is a compile-time numeric
/// constant, encode the little-endian element bytes for a passive data
/// segment. Fractional or otherwise non-trivial elements return `None` so the
/// runtime store path (with its single conversion story) handles them.
fn const_typed_array_bytes(kind: TypedArrayKind, elements: &[Expr]) -> Option<Vec<u8>> {
    fn literal_parts(expr: &Expr) -> Option<(&str, bool)> {
        match expr {
            Expr::Number(literal, _) => Some((literal.raw.as_str(), false)),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
                ..
            } => match expr.as_ref() {
                Expr::Number(literal, _) => Some((literal.raw.as_str(), true)),
                _ => None,
            },
            _ => None,
        }
    }

    fn parse_const_f64(raw: &str) -> Option<f64> {
        if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
            return u128::from_str_radix(hex, 16).ok().map(|value| value as f64);
        }
        if let Some(binary) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
            return u128::from_str_radix(binary, 2)
                .ok()
                .map(|value| value as f64);
        }
        raw.parse::<f64>().ok()
    }

    fn parse_const_integer(raw: &str) -> Option<i128> {
        if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
            return i128::from_str_radix(hex, 16).ok();
        }
        if let Some(binary) = raw.strip_prefix("0b").or_else(|| raw.strip_prefix("0B")) {
            return i128::from_str_radix(binary, 2).ok();
        }
        raw.parse::<i128>().ok()
    }

    let mut bytes = Vec::with_capacity(elements.len() * kind.element_size() as usize);
    for element in elements {
        let (raw, negative) = literal_parts(element)?;
        let raw = raw.replace('_', "");
        match kind {
            TypedArrayKind::F32 | TypedArrayKind::F64 => {
                let mut value = parse_const_f64(&raw)?;
                if negative {
                    value = -value;
                }
                match kind {
                    TypedArrayKind::F32 => bytes.extend_from_slice(&(value as f32).to_le_bytes()),
                    TypedArrayKind::F64 => bytes.extend_from_slice(&value.to_le_bytes()),
                    _ => unreachable!(),
                }
            }
            TypedArrayKind::I8
            | TypedArrayKind::U8
            | TypedArrayKind::I16
            | TypedArrayKind::U16
            | TypedArrayKind::I32
            | TypedArrayKind::U32 => {
                let mut value = parse_const_integer(&raw)?;
                if negative {
                    value = -value;
                }
                // Out-of-range constants wrap to the element width, matching
                // the runtime store's truncating behavior.
                match kind {
                    TypedArrayKind::I8 | TypedArrayKind::U8 => bytes.push(value as u8),
                    TypedArrayKind::I16 | TypedArrayKind::U16 => {
                        bytes.extend_from_slice(&(value as u16).to_le_bytes());
                    }
                    TypedArrayKind::I32 | TypedArrayKind::U32 => {
                        bytes.extend_from_slice(&(value as u32).to_le_bytes());
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    Some(bytes)
}

/// Lua `string.byte` range clipping (lstrlib.c `str_byte`): negative indices
/// count from the end of the string, the start is raised to at least 1, and
/// the end is lowered to at most the string length. Unlike a symmetric clamp,
/// a start past the end of the string (or an end before its beginning) is left
/// out of range so the caller observes an empty range and emits no values.
fn clip_lua_byte_range(start: i32, end: i32, len: i32) -> (i32, i32) {
    let posrelat = |pos: i32| {
        if pos >= 0 {
            pos
        } else {
            len.saturating_add(pos).saturating_add(1)
        }
    };
    (posrelat(start).max(1), posrelat(end).min(len))
}

fn method_signature_name(base: &str, method: &str) -> String {
    format!("{base}.{method}")
}

fn property_getter_name(base: &str, property: &str) -> String {
    format!("{base}.get/{property}")
}

fn property_setter_name(base: &str, property: &str) -> String {
    format!("{base}.set/{property}")
}

fn method_receiver_matches(expected: &Type, actual: &Type) -> bool {
    if expected == actual {
        return true;
    }
    match (expected, actual) {
        (Type::Record(expected_fields), Type::Record(actual_fields)) => expected_fields
            .iter()
            .all(|(name, expected_ty)| actual_fields.get(name) == Some(expected_ty)),
        _ => false,
    }
}

fn is_record_like(ty: &Type) -> bool {
    match ty {
        Type::Record(_) => true,
        Type::Opaque { ty, .. } => is_record_like(ty),
        Type::Nullable(ty) => is_record_like(ty),
        _ => false,
    }
}

fn method_signature(
    receiver: &Expr,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(Vec<Type>, Type)> {
    let Expr::Name(base, _, _) = receiver else {
        return None;
    };
    signatures.get(&method_signature_name(base, name)).cloned()
}

fn type_method_signature(
    receiver_ty: &Type,
    resolved_name: Option<&str>,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    if let Some(direct_name) = resolved_name {
        return signatures
            .get(direct_name)
            .cloned()
            .map(|(params, return_type)| (direct_name.to_string(), params, return_type));
    }
    if let Type::Opaque { name: type_name, .. } = receiver_ty {
        let direct_name = method_signature_name(type_name, name);
        return signatures
            .get(&direct_name)
            .cloned()
            .map(|(params, return_type)| (direct_name, params, return_type));
    }

    let suffix = format!(".{name}");
    let mut matches = signatures
        .iter()
        .filter_map(|(direct_name, (params, return_type))| {
            if !direct_name.ends_with(&suffix) {
                return None;
            }
            let receiver_param = params.first()?;
            method_receiver_matches(receiver_param, receiver_ty)
                .then(|| (direct_name.clone(), params.clone(), return_type.clone()))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn type_property_getter_signature(
    receiver_ty: &Type,
    resolved_name: Option<&str>,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    if let Some(direct_name) = resolved_name {
        if let Some((params, return_type)) = signatures.get(direct_name).cloned() {
            return Some((direct_name.to_string(), params, return_type));
        }
    }
    if let Type::Opaque { name: type_name, .. } = receiver_ty {
        let direct_name = property_getter_name(type_name, name);
        if let Some((params, return_type)) = signatures.get(&direct_name).cloned() {
            return Some((direct_name, params, return_type));
        }
    }

    if is_record_like(receiver_ty) {
        return None;
    }

    let suffix = format!(".get/{name}");
    let mut matches = signatures
        .iter()
        .filter_map(|(direct_name, (params, return_type))| {
            if !direct_name.ends_with(&suffix) || params.len() != 1 {
                return None;
            }
            let receiver_param = params.first()?;
            method_receiver_matches(receiver_param, receiver_ty)
                .then(|| (direct_name.clone(), params.clone(), return_type.clone()))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn type_property_setter_signature(
    receiver_ty: &Type,
    resolved_name: Option<&str>,
    name: &str,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    if let Some(direct_name) = resolved_name {
        if let Some((params, return_type)) = signatures.get(direct_name).cloned() {
            return Some((direct_name.to_string(), params, return_type));
        }
    }
    if let Type::Opaque { name: type_name, .. } = receiver_ty {
        let direct_name = property_setter_name(type_name, name);
        if let Some((params, return_type)) = signatures.get(&direct_name).cloned() {
            return Some((direct_name, params, return_type));
        }
    }

    if is_record_like(receiver_ty) {
        return None;
    }

    let suffix = format!(".set/{name}");
    let mut matches = signatures
        .iter()
        .filter_map(|(direct_name, (params, return_type))| {
            if !direct_name.ends_with(&suffix) || params.len() != 2 {
                return None;
            }
            let receiver_param = params.first()?;
            method_receiver_matches(receiver_param, receiver_ty)
                .then(|| (direct_name.clone(), params.clone(), return_type.clone()))
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn direct_field_call_name(
    callee: &Expr,
    signatures: &HashMap<String, (Vec<Type>, Type)>,
) -> Option<(String, Vec<Type>, Type)> {
    let Expr::Field { base, name, .. } = callee else {
        return None;
    };
    let Expr::Name(base, _, _) = base.as_ref() else {
        return None;
    };
    let direct_name = method_signature_name(base, name);
    signatures
        .get(&direct_name)
        .cloned()
        .map(|(params, return_type)| (direct_name, params, return_type))
}

impl Builder<'_> {
    /// Signature of an intrinsic protected by `pcall`, derived from the
    /// protected arguments. Each argument doubles as evidence: it selects the
    /// arity and, for the element-generic `table.*` members, the element type.
    fn intrinsic_value_signature_for_call(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<(Vec<Type>, Type), Diagnostic>> {
        let arg_types = args
            .iter()
            .map(|arg| self.infer_expr_type(arg, types, None).ok().map(first_of_multi))
            .collect::<Vec<_>>();
        let (params, return_type) =
            waluau_hir::intrinsic_values::intrinsic_value_signature_for_arguments(name, &arg_types)?;
        for (arg, param) in args.iter().zip(params.iter()) {
            if let Err(error) = self
                .infer_expr_type(arg, types, Some(param.clone()))
                .and_then(|actual| coerce_type(actual, Some(param.clone())))
            {
                return Some(Err(error));
            }
        }
        Some(Ok((params, return_type)))
    }

    /// Signature of an intrinsic referenced as a value where an explicit
    /// function type is available.
    fn intrinsic_value_signature_from_expected(
        &self,
        name: &str,
        expected: Option<&Type>,
    ) -> Option<Result<(Vec<Type>, Type), Diagnostic>> {
        let Some(Type::Function {
            params,
            return_type,
            has_self: false,
        }) = expected
        else {
            return None;
        };
        let valid = waluau_hir::intrinsic_values::intrinsic_value_signature_matches(
            name,
            params,
            return_type,
        )?;
        Some(if valid {
            Ok((params.clone(), return_type.as_ref().clone()))
        } else {
            Err(Diagnostic::new(format!(
                "function type {} does not match builtin '{name}'",
                expected.expect("matched above")
            )))
        })
    }

    fn lower_intrinsic_value(
        &mut self,
        name: &str,
        params: Vec<Type>,
        return_type: Type,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let adapter_name = format!(
            "{}$builtin_value${}${}",
            self.function.name,
            self.lambda_counter,
            name.replace('.', "$")
        );
        self.lambda_counter += 1;
        let entry = BlockId(0);
        let mut adapter = Function {
            name: adapter_name.clone(),
            params: params
                .iter()
                .enumerate()
                .map(|(index, ty)| (format!("arg{index}"), ty.clone()))
                .collect(),
            return_type: return_type.clone(),
            entry,
            blocks: BTreeMap::from([(
                entry,
                BasicBlock {
                    id: entry,
                    instructions: Vec::new(),
                    terminator: Terminator::Unreachable { span: None },
                },
            )]),
            next_value: 0,
            capture_count: 0,
            value_symbols: BTreeMap::new(),
            symbol_id: None,
            source_map: FunctionSourceMap::synthetic(),
        };
        let mut env = HashMap::new();
        let mut param_types = HashMap::new();
        let mut call_args = Vec::with_capacity(params.len());
        for (index, ty) in params.iter().enumerate() {
            let symbol_id = SymbolId(usize::MAX - index);
            let value = adapter.next_value();
            block_mut(&mut adapter, entry)
                .instructions
                .push((value, Instruction::Param(index)));
            env.insert(symbol_id, value);
            param_types.insert(symbol_id, ty.clone());
            call_args.push(Expr::Name(format!("arg{index}"), Some(symbol_id), None));
        }
        let callee = if let Some((namespace, member)) = name.split_once('.') {
            Expr::Field {
                base: Box::new(Expr::Name(namespace.to_string(), None, None)),
                name: member.to_string(),
                resolved_name: None,
                span: None,
            }
        } else {
            Expr::Name(name.to_string(), None, None)
        };
        let call = Expr::Call {
            callee: Box::new(callee),
            type_args: Vec::new(),
            args: call_args,
            span: None,
            method_call_origin: None,
        };
        let mut nested = Builder {
            function: adapter,
            current_block: entry,
            next_block: 1,
            cx: self.cx,
            lifted_functions: Vec::new(),
            lambda_counter: 0,
            loop_stack: Vec::new(),
            cell_names: HashSet::new(),
            file_path: self.file_path.clone(),
            source_file: None,
            current_span: None,
            vararg_value: None,
            vararg_element: None,
            discriminants: HashMap::new(),
            symbol_storage_types: param_types.clone(),
        };
        let mut scope = Scope {
            env,
            types: param_types,
        };
        let result = if name == ASSERT {
            // `assert` is the one intrinsic with no expression form: its
            // direct lowering is the statement that traps on a false
            // condition, so the adapter body is that statement plus a unit
            // result.
            let Expr::Call { args, span, .. } = &call else {
                unreachable!("adapter body is a call expression")
            };
            nested.lower_assert_call(args, *span, &mut scope)?;
            nested.emit(Instruction::Unit)
        } else {
            nested.lower_expr(&call, &scope, Some(return_type.clone()))?
        };
        // A diverging body (`error`) already terminated the entry block and
        // left lowering in the dead block, which has no entry to terminate.
        if nested.current_block != DEAD_BLOCK {
            nested.set_terminator(nested.current_block, Terminator::Return(result));
        }
        self.lifted_functions.push(nested.function);
        self.lifted_functions.extend(nested.lifted_functions);
        let closure = self.emit(Instruction::Closure {
            name: adapter_name,
            captures: Vec::new(),
            params: params.clone(),
            return_type: return_type.clone(),
        });
        self.coerce_value(
            closure,
            Type::Function {
                params,
                return_type: Arc::new(return_type),
                has_self: false,
            },
            expected,
        )
    }

    fn host_import_value_for_call(
        &self,
        callee: &Expr,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Option<HostImportValue>, Diagnostic> {
        let Some(display_name) = builtin_name(callee) else {
            return Ok(None);
        };
        let mut candidates = self
            .cx
            .host_import_names
            .iter()
            .filter(|&(import_name, _symbol_id)| import_name == &display_name
                    || waluau_ast::overload_base_name(import_name)
                        == Some(display_name.as_str())).map(|(import_name, symbol_id)| {
                    let (params, return_type) = self
                        .cx
                        .host_import_signatures
                        .get(symbol_id)
                        .expect("host import name has a signature");
                    (import_name.clone(), *symbol_id, params.clone(), return_type.clone())
                })
            .filter(|(_, _, params, _)| call_arity_matches(params, args.len()))
            .collect::<Vec<_>>();
        let Some(closest) = candidates
            .iter()
            .map(|(_, _, params, _)| params.len().abs_diff(args.len()))
            .min()
        else {
            return Ok(None);
        };
        candidates.retain(|(_, _, params, _)| params.len().abs_diff(args.len()) == closest);
        let mut viable = Vec::new();
        for candidate in candidates {
            let mut coercion_cost = 0;
            let mut matches = true;
            for (arg, param) in args.iter().zip(candidate.2.iter()) {
                let unconstrained = self.infer_expr_type(arg, types, None).ok();
                if unconstrained.as_ref() == Some(param) {
                    continue;
                }
                if self
                    .infer_expr_type(arg, types, Some(param.clone()))
                    .and_then(|actual| coerce_type(actual, Some(param.clone())))
                    .is_ok()
                {
                    coercion_cost += 1;
                } else {
                    matches = false;
                    break;
                }
            }
            if matches {
                viable.push((coercion_cost, candidate));
            }
        }
        viable.sort_by(|left, right| (left.0, &left.1.0).cmp(&(right.0, &right.1.0)));
        let Some((best_cost, best)) = viable.first() else {
            return Ok(None);
        };
        if viable
            .get(1)
            .is_some_and(|(cost, _)| cost == best_cost)
        {
            return Err(Diagnostic::new(format!(
                "protected call does not uniquely select an overload of '{display_name}'"
            )));
        }
        Ok(Some(best.clone()))
    }

    /// Materialize a declared browser-host import as an ordinary Waluau
    /// function value. Direct calls keep their existing `HostCall` lowering;
    /// only a value reference pays for this private adapter.
    fn lower_host_import_value(
        &mut self,
        display_name: &str,
        symbol_id: SymbolId,
        params: Vec<Type>,
        return_type: Type,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let adapter_name = format!(
            "{}$host_value${}${}",
            self.function.name,
            self.lambda_counter,
            display_name.replace('.', "$")
        );
        self.lambda_counter += 1;
        let entry = BlockId(0);
        let mut adapter = Function {
            name: adapter_name.clone(),
            params: params
                .iter()
                .enumerate()
                .map(|(index, ty)| (format!("arg{index}"), ty.clone()))
                .collect(),
            return_type: return_type.clone(),
            entry,
            blocks: BTreeMap::new(),
            next_value: 0,
            capture_count: 0,
            value_symbols: BTreeMap::new(),
            symbol_id: None,
            source_map: FunctionSourceMap::synthetic(),
        };
        let mut instructions = Vec::with_capacity(params.len() + 1);
        let mut args = Vec::with_capacity(params.len());
        for index in 0..params.len() {
            let value = adapter.next_value();
            instructions.push((value, Instruction::Param(index)));
            args.push(value);
        }
        let result = adapter.next_value();
        instructions.push((
            result,
            Instruction::HostCall {
                name: display_name.to_string(),
                symbol_id,
                args,
                return_type: return_type.clone(),
            },
        ));
        adapter.blocks.insert(
            entry,
            BasicBlock {
                id: entry,
                instructions,
                terminator: Terminator::Return(result),
            },
        );
        self.lifted_functions.push(adapter);
        let closure = self.emit(Instruction::Closure {
            name: adapter_name,
            captures: Vec::new(),
            params: params.clone(),
            return_type: return_type.clone(),
        });
        self.coerce_value(
            closure,
            Type::Function {
                params,
                return_type: Arc::new(return_type),
                has_self: false,
            },
            expected,
        )
    }

    fn function_expr_return_type(function: &waluau_ast::FunctionExpr) -> Result<Type, Diagnostic> {
        function.return_type.clone().ok_or_else(|| {
            Diagnostic::new(
                "function return inference is only supported for named functions in this MVP",
            )
        })
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        self.function.blocks.insert(
            id,
            BasicBlock {
                id,
                instructions: Vec::new(),
                terminator: Terminator::Unreachable { span: None },
            },
        );
        id
    }

    fn emit(&mut self, mut instruction: Instruction) -> ValueId {
        sort_phi_incoming(&mut instruction);
        canonicalize_cast_types(&mut instruction);
        let value = self.function.next_value();
        block_mut(&mut self.function, self.current_block)
            .instructions
            .push((value, instruction));
        if let (Some(span), Some(file)) = (self.current_span, self.source_file) {
            self.function.source_map.record_instruction(
                value,
                SourceLocation { file, span },
            );
        }
        value
    }

    /// Bind a loop variable into the body environment. When a nested closure
    /// captures it, box the per-iteration value into a fresh 1-element cell
    /// (fresh per iteration, matching Luau's per-iteration bindings) so the
    /// closure shares storage with body reads.
    fn bind_loop_var(
        &mut self,
        symbol_id: SymbolId,
        value: ValueId,
        ty: &Type,
        body_env: &mut HashMap<SymbolId, ValueId>,
    ) {
        if self.cell_names.contains(&symbol_id) {
            let cell = self.emit(Instruction::ArrayNew {
                element_ty: to_runtime_type(ty),
                elements: vec![value],
            });
            body_env.insert(symbol_id, cell);
            self.function.value_symbols.insert(cell, symbol_id);
        } else {
            body_env.insert(symbol_id, value);
        }
        self.function.value_symbols.insert(value, symbol_id);
        self.symbol_storage_types.insert(symbol_id, ty.clone());
    }

    fn bind_local_value(
        &mut self,
        symbol_id: SymbolId,
        value: ValueId,
        ty: Type,
        scope: &mut Scope,
    ) {
        if self.cell_names.contains(&symbol_id) {
            let cell = self.emit(Instruction::ArrayNew {
                element_ty: to_runtime_type(&ty),
                elements: vec![value],
            });
            scope.env.insert(symbol_id, cell);
            self.function.value_symbols.insert(cell, symbol_id);
        } else {
            scope.env.insert(symbol_id, value);
        }
        self.function.value_symbols.insert(value, symbol_id);
        self.symbol_storage_types.insert(symbol_id, ty.clone());
        scope.types.insert(symbol_id, ty);
    }

    fn assign_local_value(
        &mut self,
        symbol_id: SymbolId,
        value: ValueId,
        ty: &Type,
        env: &mut HashMap<SymbolId, ValueId>,
    ) -> Result<(), Diagnostic> {
        if self.cell_names.contains(&symbol_id) {
            let cell = env.get(&symbol_id).copied().ok_or_else(|| {
                Diagnostic::new(format!(
                    "unknown local symbol {:?} during IR lowering",
                    symbol_id
                ))
            })?;
            let index0 = self.emit(Instruction::Number {
                ty: NumericType::I32,
                literal: NumberLiteral { raw: "0".into() },
            });
            self.emit(Instruction::ArraySet {
                array: cell,
                index: index0,
                value,
                element_ty: to_runtime_type(ty),
            });
        } else {
            env.insert(symbol_id, value);
        }
        self.function.value_symbols.insert(value, symbol_id);
        Ok(())
    }

    /// Emit an instruction into a specific block (used for loop exit phis,
    /// which are created before the exit block becomes the current block).
    fn emit_in(&mut self, block: BlockId, mut instruction: Instruction) -> ValueId {
        sort_phi_incoming(&mut instruction);
        canonicalize_cast_types(&mut instruction);
        let value = self.function.next_value();
        block_mut(&mut self.function, block)
            .instructions
            .push((value, instruction));
        if let (Some(span), Some(file)) = (self.current_span, self.source_file) {
            self.function.source_map.record_instruction(
                value,
                SourceLocation { file, span },
            );
        }
        value
    }

    /// Create one phi in `exit` per mutated loop variable. The phis start with
    /// no incoming edges; each loop adds its normal exit edge once it is built,
    /// and every `break` site adds its own incoming edge via `lower_break`.
    /// Code after the loop reads these exit phis, not the header phis, so break
    /// paths observe the values live at the break.
    fn create_exit_phis(
        &mut self,
        exit: BlockId,
        phis: &HashMap<SymbolId, ValueId>,
    ) -> HashMap<SymbolId, ValueId> {
        let mut exit_phis = HashMap::new();
        // Sorted so exit-phi value ids are assigned in a deterministic order
        // regardless of the map's per-process hash seed; the artifact must be
        // byte-identical across builds.
        let mut ids: Vec<SymbolId> = phis.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let exit_phi = self.emit_in(exit, Instruction::Phi(Vec::new()));
            self.function.value_symbols.insert(exit_phi, id);
            exit_phis.insert(id, exit_phi);
        }
        exit_phis
    }

    fn lower_resolved_operator_call(
        &mut self,
        name: &str,
        args: &[&Expr],
        scope: &Scope,
    ) -> Result<(ValueId, Type), Diagnostic> {
        let (param_types, return_type) = self
            .cx
            .field_call_signatures
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("operator overload '{name}' is not declared")))?;
        if param_types.len() != args.len() {
            return Err(Diagnostic::new(format!(
                "operator overload '{name}' expects {} operands, got {}",
                param_types.len(),
                args.len()
            )));
        }
        let lowered_args = args
            .iter()
            .zip(param_types.iter())
            .map(|(arg, param_ty)| self.lower_expr(arg, scope, Some(param_ty.clone())))
            .collect::<Result<Vec<_>, _>>()?;
        let value = if let Some(symbol_id) = self.cx.host_import_names.get(name).copied() {
            self.emit(Instruction::HostCall {
                name: name.to_string(),
                symbol_id,
                args: lowered_args,
                return_type: return_type.clone(),
            })
        } else {
            self.emit(Instruction::Call {
                name: name.to_string(),
                symbol_id: None,
                args: lowered_args,
            })
        };
        Ok((value, return_type))
    }

    /// Lower every explicit argument to preserve source-order side effects,
    /// discard values beyond the fixed parameter list, and materialize omitted
    /// trailing nullable parameters as typed null values. Wasm calls always
    /// retain their declared arity.
    fn lower_fixed_call_args(
        &mut self,
        args: &[Expr],
        param_types: &[Type],
        scope: &Scope,
    ) -> Result<Vec<ValueId>, Diagnostic> {
        let mut lowered = self.lower_expr_list(args, scope, Some(param_types))?;
        if !call_arity_matches(param_types, lowered.len()) {
            return Err(Diagnostic::new(format!(
                "function expects {} arguments, got {}",
                param_types.len(),
                lowered.len()
            )));
        }
        lowered.truncate(param_types.len());
        for param_ty in &param_types[lowered.len()..] {
            lowered.push(self.emit_omitted_nullable_arg(param_ty)?);
        }
        Ok(lowered)
    }

    fn emit_omitted_nullable_arg(&mut self, param_ty: &Type) -> Result<ValueId, Diagnostic> {
        let Type::Nullable(inner) = param_ty else {
            return Err(Diagnostic::new(format!(
                "cannot omit non-nullable argument of type {param_ty}"
            )));
        };
        let runtime_ty = if param_ty.is_boxed_nullable() {
            param_ty.clone()
        } else {
            (**inner).clone()
        };
        let null = self.emit(Instruction::Null {
            ty: runtime_ty.clone(),
        });
        self.coerce_value(null, runtime_ty, Some(param_ty.clone()))
    }

    /// Materialize Lua's implicit trailing nil values after every authored RHS
    /// expression has been lowered. The caller supplies the target types so a
    /// nullable or dynamic slot gets the correct Wasm representation; HIR has
    /// already rejected a missing value for a concrete non-null target.
    fn pad_lowered_values_with_nil(
        &mut self,
        lowered: &mut Vec<ValueId>,
        expected: &[Type],
        scope: &Scope,
    ) -> Result<(), Diagnostic> {
        for expected_ty in &expected[lowered.len().min(expected.len())..] {
            lowered.push(self.lower_expr(
                &Expr::Nil(None),
                scope,
                Some(expected_ty.clone()),
            )?);
        }
        Ok(())
    }

    /// Return the stable i32 discriminant for a tagged-union variant name.
    fn variant_tag_id(&self, name: &str) -> Result<i32, Diagnostic> {
        self.cx.tag_ids
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown tagged-union variant '{name}'")))
    }

    /// Unbox the payload of a canonical tagged-union record (`{ tag: i32, value: anyref }`):
    /// reads the `value` field and casts the resulting `unknown` to the payload type.
    fn unbox_tagged_variant_value(
        &mut self,
        base: ValueId,
        payload_ty: &Type,
    ) -> Result<ValueId, Diagnostic> {
        if matches!(payload_ty, Type::String | Type::Bytes) {
            return Err(Diagnostic::new(format!(
                "reading .value of type {payload_ty} is not yet supported \
                 (string/bytes payloads cannot be unboxed from anyref)"
            )));
        }
        let unknown_val = self.emit(Instruction::StructGet {
            base,
            field: "value".to_string(),
            field_ty: Type::Unknown,
        });
        Ok(self.emit(Instruction::Cast {
            value: unknown_val,
            from: Type::Unknown,
            to: payload_ty.clone(),
        }))
    }

    fn instruction(&self, value: ValueId) -> Option<&Instruction> {
        self.function
            .blocks
            .values()
            .flat_map(|block| block.instructions.iter())
            .find_map(|(id, instruction)| (*id == value).then_some(instruction))
    }

    fn direct_record_field_closure_name(&self, base: ValueId, field: &str) -> Option<String> {
        let Instruction::StructNew { struct_ty, fields } = self.instruction(base)? else {
            return None;
        };
        let Type::Record(record_fields) = struct_ty else {
            return None;
        };
        let field_index = record_fields.keys().position(|name| name == field)?;
        let field_value = *fields.get(field_index)?;
        let Instruction::Closure { name, captures, .. } = self.instruction(field_value)? else {
            return None;
        };
        captures.is_empty().then(|| name.clone())
    }

    fn set_terminator(&mut self, block: BlockId, terminator: Terminator) {
        block_mut(&mut self.function, block).terminator = terminator;
        if let (Some(span), Some(file)) = (self.current_span, self.source_file) {
            self.function.source_map.record_terminator(
                block,
                SourceLocation { file, span },
            );
        }
    }

    fn lower_break(&mut self, env: &HashMap<SymbolId, ValueId>) -> Result<(), Diagnostic> {
        let Some(loop_ctx) = self.loop_stack.last().cloned() else {
            return Err(Diagnostic::new("break is only allowed inside loops"));
        };
        if self.current_block == DEAD_BLOCK {
            return Ok(());
        }
        let current = self.current_block;
        // Feed the values live at the break into the exit phis so code after
        // the loop observes this iteration's mutations.
        for (id, exit_phi) in &loop_ctx.exit_phis {
            let value = env
                .get(id)
                .copied()
                .or_else(|| loop_ctx.phis.get(id).copied());
            if let Some(value) = value {
                add_phi_incoming(
                    &mut self.function,
                    loop_ctx.break_target,
                    *exit_phi,
                    (current, value),
                );
            }
        }
        self.set_terminator(current, Terminator::Jump(loop_ctx.break_target));
        self.current_block = DEAD_BLOCK;
        Ok(())
    }

    fn lower_continue(&mut self, env: &HashMap<SymbolId, ValueId>) -> Result<(), Diagnostic> {
        let Some(loop_ctx) = self.loop_stack.last().cloned() else {
            return Err(Diagnostic::new("continue is only allowed inside loops"));
        };
        if self.current_block == DEAD_BLOCK {
            return Ok(());
        }
        let current = self.current_block;
        for (id, phi) in &loop_ctx.phis {
            if let Some(value) = env.get(id).copied() {
                add_phi_incoming(&mut self.function, loop_ctx.header, *phi, (current, value));
            }
        }
        // Replay the implicit loop-carried updates (loop index, bounds) so the
        // header phis gain an incoming edge for this continue predecessor, just
        // as they would when control falls off the bottom of the body.
        for carry in &loop_ctx.carries {
            let value = match &carry.update {
                CarryUpdate::Invariant(value) => *value,
                CarryUpdate::Increment { base, step, ty } => self.emit(Instruction::Binary {
                    op: BinaryOp::Add,
                    left: *base,
                    right: *step,
                    operand_ty: ty.clone(),
                    result_ty: ty.clone(),
                }),
            };
            add_phi_incoming(&mut self.function, loop_ctx.header, carry.phi, (current, value));
        }
        self.set_terminator(current, Terminator::Jump(loop_ctx.continue_target));
        self.current_block = DEAD_BLOCK;
        Ok(())
    }

    fn lower_stmt(
        &mut self,
        stmt: &Stmt,
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let previous = std::mem::replace(&mut self.current_span, primary_stmt_span(stmt));
        let result = self.lower_stmt_inner(stmt, scope);
        self.current_span = previous;
        result
    }

    fn lower_stmt_inner(
        &mut self,
        stmt: &Stmt,
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Let {
                name: _,
                symbol_id,
                rebindability: _,
                ty,
                value,
            } => {
                let symbol_id = symbol_id.expect("symbol_id should be resolved");
                let inferred_ty = if let Some(ty) = ty.clone() {
                    ty
                } else if matches!(value, Expr::ArrayLiteral { elements, .. } if elements.is_empty()) {
                    // Keep parity with HIR inference: bare `{}` in local initialization starts
                    // as an empty record so subsequent `t.field = ...` can shape it.
                    Type::record(BTreeMap::new())
                } else {
                    // A multi-value initializer collapses to its first value.
                    match self.infer_expr_type(value, &scope.types, None)? {
                        Type::Multi(mut parts) if !parts.is_empty() => parts.remove(0),
                        other => other,
                    }
                };
                let lexical_function_declaration = stmt.lexical_function_declaration().is_some();
                if lexical_function_declaration && !self.cx.globals.contains_key(&symbol_id) {
                    // A non-global closure captures its own lexical binding.
                    // Allocate that binding's cell first, then append the
                    // closure after it has captured the cell. Rebinding updates
                    // the same cell observed by calls from the original
                    // closure. Module-visible bindings use the existing global
                    // path below, which provides the same shared-storage
                    // semantics across top-level and module functions.
                    self.cell_names.insert(symbol_id);
                    let cell = self.emit(Instruction::ArrayNew {
                        element_ty: to_runtime_type(&inferred_ty),
                        elements: Vec::new(),
                    });
                    scope.env.insert(symbol_id, cell);
                    scope.types.insert(symbol_id, inferred_ty.clone());
                    self.symbol_storage_types
                        .insert(symbol_id, inferred_ty.clone());
                    self.function.value_symbols.insert(cell, symbol_id);

                    let value =
                        self.lower_expr(value, scope, Some(inferred_ty.clone()))?;
                    let index0 = self.emit_i32_const(0);
                    self.emit(Instruction::ArraySet {
                        array: cell,
                        index: index0,
                        value,
                        element_ty: to_runtime_type(&inferred_ty),
                    });
                    self.function.value_symbols.insert(value, symbol_id);
                    return Ok(());
                }
                let value = self.lower_expr(value, scope, Some(inferred_ty.clone()))?;
                if let Some((global, _)) = self.cx.globals.get(&symbol_id) {
                    self.emit(Instruction::GlobalSet {
                        global: *global,
                        value,
                        ty: to_runtime_type(&inferred_ty),
                    });
                } else {
                    // If this local is captured by any nested function, represent it as a 1-element
                    // array cell so closures can observe and mutate the same storage location.
                    self.bind_local_value(symbol_id, value, inferred_ty, scope);
                }
            }
            Stmt::Assign { op, name, symbol_id, value } => {
                let symbol_id = symbol_id.expect("symbol_id should be resolved");
                let ty = scope.types.get(&symbol_id).cloned().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                })?;
                if let Some((global, _)) = self.cx.globals.get(&symbol_id).cloned() {
                    let stored = match op {
                        AssignOp::Set => self.lower_expr(value, scope, Some(ty.clone()))?,
                        AssignOp::Compound(bin_op) => {
                            if !bin_op.compound_target_ok(&ty) {
                                return Err(Diagnostic::new(format!(
                                    "compound assignment to '{}' requires a {} target",
                                    name,
                                    bin_op.compound_target_kind()
                                )));
                            }
                            let current = self.emit(Instruction::GlobalGet {
                                global,
                                ty: to_runtime_type(&ty),
                            });
                            let rhs =
                                self.lower_compound_rhs(*bin_op, value, &ty, scope)?;
                            self.emit(Instruction::Binary {
                                op: *bin_op,
                                left: current,
                                right: rhs,
                                operand_ty: ty.clone(),
                                result_ty: ty.clone(),
                            })
                        }
                    };
                    self.emit(Instruction::GlobalSet {
                        global,
                        value: stored,
                        ty: to_runtime_type(&ty),
                    });
                } else if self.cell_names.contains(&symbol_id) {
                    // Captured local: stored in a 1-element array (cell). Perform ArraySet
                    // rather than rebinding the env entry.
                    let cell = scope.env.get(&symbol_id).copied().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                    })?;
                    let index0 = self.emit(Instruction::Number {
                        ty: NumericType::I32,
                        literal: NumberLiteral { raw: "0".into() },
                    });
                    match op {
                        AssignOp::Set => {
                            let rhs = self.lower_expr(value, scope, Some(ty.clone()))?;
                            self.function.value_symbols.insert(rhs, symbol_id);
                            self.emit(Instruction::ArraySet {
                                array: cell,
                                index: index0,
                                value: rhs,
                                element_ty: to_runtime_type(&ty),
                            });
                        }
                        AssignOp::Compound(bin_op) => {
                            if !bin_op.compound_target_ok(&ty) {
                                return Err(Diagnostic::new(format!(
                                    "compound assignment to '{}' requires a {} target",
                                    name,
                                    bin_op.compound_target_kind()
                                )));
                            }
                            // load current, combine, store back
                            let current = self.emit(Instruction::ArrayGet {
                                array: cell,
                                index: index0,
                                element_ty: ty.clone(),
                            });
                            let rhs =
                                self.lower_compound_rhs(*bin_op, value, &ty, scope)?;
                            let sum = self.emit(Instruction::Binary {
                                op: *bin_op,
                                left: current,
                                right: rhs,
                                operand_ty: ty.clone(),
                                result_ty: ty.clone(),
                            });
                            self.function.value_symbols.insert(sum, symbol_id);
                            self.emit(Instruction::ArraySet {
                                array: cell,
                                index: index0,
                                value: sum,
                                element_ty: ty.clone(),
                            });
                        }
                    }
                    // Do not replace env entry -- it remains the cell.
                } else {
                    let value = match op {
                        AssignOp::Set => self.lower_expr(value, scope, Some(ty))?,
                        AssignOp::Compound(bin_op) => {
                            if !bin_op.compound_target_ok(&ty) {
                                return Err(Diagnostic::new(format!(
                                    "compound assignment to '{}' requires a {} target",
                                    name,
                                    bin_op.compound_target_kind()
                                )));
                            }
                            let current = *scope.env.get(&symbol_id).ok_or_else(|| {
                                Diagnostic::new(format!(
                                    "unknown local '{name}' during IR lowering"
                                ))
                            })?;
                            let rhs =
                                self.lower_compound_rhs(*bin_op, value, &ty, scope)?;
                            self.emit(Instruction::Binary {
                                op: *bin_op,
                                left: current,
                                right: rhs,
                                operand_ty: ty.clone(),
                                result_ty: ty,
                            })
                        }
                    };
                    scope.env.insert(symbol_id, value);
                    self.function.value_symbols.insert(value, symbol_id);
                }
                self.sever_discriminants(symbol_id);
            }
            Stmt::IndexAssign {
                op,
                base,
                index,
                value,
            } => {
                let base_ty = self.infer_expr_type(base, &scope.types, None)?;
                if let Type::TypedArray(kind) = &base_ty {
                    let kind = *kind;
                    let element_ty = Type::Numeric(kind.element_numeric_type());
                    let buffer = self.lower_expr(base, scope, Some(base_ty.clone()))?;
                    let index = self.lower_index_to_i32(index, scope)?;
                    let stored = match op {
                        AssignOp::Set => {
                            self.lower_numeric_to(value, &element_ty, scope)?
                        }
                        AssignOp::Compound(bin_op) => {
                            if !bin_op.compound_target_ok(&element_ty) {
                                return Err(Diagnostic::new(format!(
                                    "compound array assignment requires {} elements",
                                    bin_op.compound_target_kind()
                                )));
                            }
                            let current = self.emit(Instruction::BufferGet {
                                buffer,
                                index,
                                kind,
                            });
                            let rhs = self.lower_numeric_to(value, &element_ty, scope)?;
                            self.emit(Instruction::Binary {
                                op: *bin_op,
                                left: current,
                                right: rhs,
                                operand_ty: element_ty.clone(),
                                result_ty: element_ty.clone(),
                            })
                        }
                    };
                    self.emit(Instruction::BufferSet {
                        buffer,
                        index,
                        value: stored,
                        kind,
                    });
                    return Ok(());
                }
                let element_ty = base_ty.element_type().ok_or_else(|| {
                    Diagnostic::new("array element assignment requires an array operand")
                })?;
                let array = self.lower_expr(base, scope, Some(base_ty))?;
                let index = self.lower_authored_array_index(index, scope)?;
                let value = match op {
                    AssignOp::Set => {
                        self.lower_expr(value, scope, Some(element_ty.clone()))?
                    }
                    AssignOp::Compound(bin_op) => {
                        if !bin_op.compound_target_ok(&element_ty) {
                            return Err(Diagnostic::new(format!(
                                "compound array assignment requires {} elements",
                                bin_op.compound_target_kind()
                            )));
                        }
                        let current = self.emit(Instruction::ArrayGet {
                            array,
                            index,
                            element_ty: element_ty.clone(),
                        });
                        let rhs = self.lower_compound_rhs(
                            *bin_op,
                            value,
                            &element_ty,
                            scope,
                        )?;
                        self.emit(Instruction::Binary {
                            op: *bin_op,
                            left: current,
                            right: rhs,
                            operand_ty: element_ty.clone(),
                            result_ty: element_ty.clone(),
                        })
                    }
                };
                self.emit(Instruction::ArraySet {
                    array,
                    index,
                    value,
                    element_ty,
                });
            }
            Stmt::FieldAssign { .. } => {
                let Stmt::FieldAssign {
                    op,
                    base,
                    name,
                    resolved_name,
                    value,
                } = stmt
                else {
                    unreachable!();
                };
                let base_ty = self.infer_expr_type(base, &scope.types, None)?;
                if let Some((setter_name, params, return_type)) =
                    type_property_setter_signature(
                        &base_ty,
                        resolved_name.as_deref(),
                        name,
                        self.cx.field_call_signatures,
                    )
                {
                    if *op != AssignOp::Set {
                        return Err(Diagnostic::new(
                            "compound property assignment is not supported",
                        ));
                    }
                    if params.len() != 2 || !method_receiver_matches(&params[0], &base_ty) {
                        return Err(Diagnostic::new(format!(
                            "property setter for '{name}' does not accept receiver {base_ty}"
                        )));
                    }
                    if return_type != Type::Unit {
                        return Err(Diagnostic::new(format!(
                            "property setter for '{name}' must return unit"
                        )));
                    }
                    let receiver =
                        self.lower_expr(base, scope, Some(params[0].clone()))?;
                    let stored = self.lower_expr(value, scope, Some(params[1].clone()))?;
                    let symbol_id = self.cx.host_import_names.get(&setter_name).copied().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "declared property setter '{setter_name}' is missing a host import symbol"
                        ))
                    })?;
                    self.emit(Instruction::HostCall {
                        name: setter_name,
                        symbol_id,
                        args: vec![receiver, stored],
                        return_type,
                    });
                    return Ok(());
                }
                let (base_ty, field_ty) = if let Expr::Name(_base_name, Some(base_symbol_id), _) = base.as_ref() {
                    let Some(Type::Record(mut fields)) = scope.types.get(base_symbol_id).cloned() else {
                        return Err(Diagnostic::new("field assignment requires a record base"));
                    };
                    let existing_field = fields.get(name).cloned();
                    match existing_field {
                        Some(existing) => (Type::Record(fields), existing),
                        None => {
                            let inferred = self.infer_expr_type(value, &scope.types, None)?;
                            let previous_ty = Type::Record(fields.clone());
                            Arc::make_mut(&mut fields).insert(name.clone(), inferred.clone());
                            let updated_ty = Type::Record(fields.clone());
                            let base_value =
                                self.lower_expr(base, scope, Some(previous_ty.clone()))?;
                            let new_field_value =
                                self.lower_expr(value, scope, Some(inferred.clone()))?;

                            let mut lowered_fields = Vec::with_capacity(fields.len());
                            for (field_name, field_ty) in fields.iter() {
                                if field_name == name {
                                    lowered_fields.push(new_field_value);
                                } else {
                                    lowered_fields.push(self.emit(Instruction::StructGet {
                                        base: base_value,
                                        field: field_name.clone(),
                                        field_ty: field_ty.clone(),
                                    }));
                                }
                            }
                            let rebuilt = self.emit(Instruction::StructNew {
                                struct_ty: updated_ty.clone(),
                                fields: lowered_fields,
                            });
                            scope.env.insert(*base_symbol_id, rebuilt);
                            self.function.value_symbols.insert(rebuilt, *base_symbol_id);
                            self.symbol_storage_types
                                .insert(*base_symbol_id, updated_ty.clone());
                            scope.types.insert(*base_symbol_id, updated_ty);
                            return Ok(());
                        }
                    }
                } else {
                    let field_ty = base_ty.record_field(name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown record field '{name}'"))
                    })?;
                    (base_ty, field_ty)
                };
                let base = self.lower_expr(base, scope, Some(base_ty))?;
                let value = match op {
                    AssignOp::Set => self.lower_expr(value, scope, Some(field_ty.clone()))?,
                    AssignOp::Compound(bin_op) => {
                        if !bin_op.compound_target_ok(&field_ty) {
                            return Err(Diagnostic::new(format!(
                                "compound field assignment requires a {} field",
                                bin_op.compound_target_kind()
                            )));
                        }
                        let current = self.emit(Instruction::StructGet {
                            base,
                            field: name.clone(),
                            field_ty: field_ty.clone(),
                        });
                        let rhs = self.lower_compound_rhs(
                            *bin_op,
                            value,
                            &field_ty,
                            scope,
                        )?;
                        self.emit(Instruction::Binary {
                            op: *bin_op,
                            left: current,
                            right: rhs,
                            operand_ty: field_ty.clone(),
                            result_ty: field_ty.clone(),
                        })
                    }
                };
                self.emit(Instruction::StructSet {
                    base,
                    field: name.clone(),
                    value,
                });
            }
            Stmt::Expr(expr) => {
                if let Expr::Call {
                    callee,
                    args,
                    span,
                    ..
                } = expr
                {
                    if let Expr::Name(name, _, _) = callee.as_ref() {
                        if name == ASSERT {
                            self.lower_assert_call(args, *span, scope)?;
                            // Code after `assert(cond)` only runs when the
                            // condition held: apply the then-branch narrowing.
                            self.apply_assert_narrowing(&args[0], scope);
                            return Ok(());
                        }
                    }
                }
                let _ = self.lower_expr(expr, scope, None)?;
            }
            Stmt::Break => {
                self.lower_break(&scope.env)?;
            }
            Stmt::Continue => {
                self.lower_continue(&scope.env)?;
            }
            Stmt::Return(expr) => {
                let value = if matches!(expr, Expr::Nil(_))
                    && self.function.return_type == Type::Unit
                {
                    self.emit(Instruction::Unit)
                } else {
                    self.lower_expr(expr, scope, Some(self.function.return_type.clone()))?
                };
                self.set_terminator(self.current_block, Terminator::Return(value));
                self.current_block = DEAD_BLOCK;
            }
            Stmt::ReturnMulti(values) => {
                let expected = match &self.function.return_type {
                    Type::Multi(tys) => tys.clone(),
                    other => vec![other.clone()],
                };
                let lowered = self.lower_expr_list(values, scope, Some(&expected))?;
                if lowered.len() != expected.len() {
                    return Err(Diagnostic::new(format!(
                        "return expects {} values, got {}",
                        expected.len(),
                        lowered.len()
                    )));
                }
                let packed = self.emit(Instruction::PackMulti {
                    values: lowered,
                    types: expected,
                });
                self.set_terminator(self.current_block, Terminator::Return(packed));
                self.current_block = DEAD_BLOCK;
            }
            Stmt::LetMulti { bindings, values } => {
                let any_typed = bindings.iter().any(|binding| binding.ty.is_some());
                if any_typed {
                    // Annotated names use their annotation; unannotated names
                    // in a mixed declaration fall back to the unconstrained
                    // type of their initializer slot.
                    let expected: Vec<Type> = if bindings.iter().all(|binding| binding.ty.is_some())
                    {
                        bindings
                            .iter()
                            .map(|binding| binding.ty.clone().expect("checked above"))
                            .collect()
                    } else {
                        let mut inferred_types = Vec::new();
                        for expr in values {
                            let ty = self.infer_expr_type(expr, &scope.types, None)?;
                            match ty {
                                Type::Multi(types_for_expr) => {
                                    inferred_types.extend(types_for_expr);
                                }
                                other => inferred_types.push(other),
                            }
                        }
                        if inferred_types.len() < bindings.len() {
                            return Err(Diagnostic::new(format!(
                                "multi-binding declaration expects {} values, got {}",
                                bindings.len(),
                                inferred_types.len()
                            )));
                        }
                        bindings
                            .iter()
                            .zip(inferred_types)
                            .map(|(binding, inferred)| binding.ty.clone().unwrap_or(inferred))
                            .collect()
                    };
                    let mut lowered = self.lower_expr_list(values, scope, Some(&expected))?;
                    self.pad_lowered_values_with_nil(&mut lowered, &expected, scope)?;
                    for ((binding, value), expected_ty) in
                        bindings.iter().zip(lowered).zip(expected)
                    {
                        let symbol_id = binding.symbol_id.expect("resolved symbol_id");
                        self.bind_local_value(symbol_id, value, expected_ty, scope);
                    }
                } else {
                    let mut inferred_types = Vec::new();
                    for expr in values {
                        let ty = self.infer_expr_type(expr, &scope.types, None)?;
                        match ty {
                            Type::Multi(types_for_expr) => {
                                inferred_types.extend(types_for_expr);
                            }
                            other => inferred_types.push(other),
                        }
                    }
                    // Extra explicit values and trailing multi-return slots
                    // are dropped only after lowering, following Lua's
                    // adjustment and side-effect rules.
                    while inferred_types.len() < bindings.len() {
                        inferred_types.push(Type::Nil);
                    }
                    let mut lowered =
                        self.lower_expr_list(values, scope, Some(&inferred_types))?;
                    self.pad_lowered_values_with_nil(&mut lowered, &inferred_types, scope)?;
                    for ((binding, value), ty) in bindings.iter().zip(lowered).zip(inferred_types) {
                        let symbol_id = binding.symbol_id.expect("resolved symbol_id");
                        self.bind_local_value(symbol_id, value, ty, scope);
                    }
                }
                self.record_pcall_discriminant(bindings, values, &scope.types);
            }
            Stmt::AssignMulti { targets, symbol_ids, values } => {
                let ids = symbol_ids.as_ref().expect("symbol_ids should be resolved");
                let mut expected = Vec::new();
                for (target, id) in targets.iter().zip(ids) {
                    let ty = scope.types.get(id).cloned().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{target}' during IR lowering"))
                    })?;
                    expected.push(ty);
                }
                let mut lowered = self.lower_expr_list(values, scope, Some(&expected))?;
                self.pad_lowered_values_with_nil(&mut lowered, &expected, scope)?;
                for ((id, value), ty) in ids.iter().zip(lowered).zip(expected.iter()) {
                    self.assign_local_value(*id, value, ty, &mut scope.env)?;
                    self.sever_discriminants(*id);
                }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.lower_if(condition, then_body, else_body, scope)?;
            }
            Stmt::Match {
                value,
                enum_ty,
                arms,
            } => {
                // HIR has already established that every variant appears once.
                // Bind the scrutinee to an impossible source-level symbol so it
                // is evaluated exactly once, then reuse the ordinary if/phi
                // lowering for the exhaustive dispatch tree.
                let match_symbol = SymbolId(usize::MAX);
                let saved_value = scope.env.insert(
                    match_symbol,
                    self.lower_expr(value, scope, Some(enum_ty.clone()))?,
                );
                let saved_type = scope.types.insert(match_symbol, enum_ty.clone());
                let mut otherwise = arms
                    .last()
                    .expect("parser rejects empty matches")
                    .body
                    .clone();
                for arm in arms[..arms.len() - 1].iter().rev() {
                    let condition = Expr::Binary {
                        op: BinaryOp::Eq,
                        left: Box::new(Expr::Name(
                            "$match".to_string(),
                            Some(match_symbol),
                            None,
                        )),
                        right: Box::new(Expr::Cast {
                            expr: Box::new(Expr::Number(
                                NumberLiteral {
                                    raw: arm.ordinal.to_string(),
                                },
                                None,
                            )),
                            ty: enum_ty.clone(),
                            span: None,
                        }),
                        resolved_name: None,
                        span: None,
                    };
                    otherwise = vec![Stmt::If {
                        condition,
                        then_body: arm.body.clone(),
                        else_body: otherwise,
                    }];
                }
                for stmt in &otherwise {
                    if self.current_block == DEAD_BLOCK {
                        break;
                    }
                    self.lower_stmt(stmt, scope)?;
                }
                match saved_value {
                    Some(value) => {
                        scope.env.insert(match_symbol, value);
                    }
                    None => {
                        scope.env.remove(&match_symbol);
                    }
                }
                match saved_type {
                    Some(ty) => {
                        scope.types.insert(match_symbol, ty);
                    }
                    None => {
                        scope.types.remove(&match_symbol);
                    }
                }
            }
            Stmt::IfCast {
                target_name,
                binding_symbol_id,
                value,
                then_body,
                else_body,
                ..
            } => {
                self.lower_if_cast(
                    IfCastParts {
                        target_name,
                        binding_symbol_id: *binding_symbol_id,
                        value,
                        then_body,
                        else_body,
                    },
                    scope,
                )?;
            }
            Stmt::While { condition, body } => {
                self.lower_while(condition, body, scope)?;
            }
            Stmt::Repeat { body, condition } => {
                self.lower_repeat(body, condition, scope)?;
            }
            Stmt::NumericFor {
                symbol_id,
                start,
                stop,
                step,
                body,
                ..
            } => {
                self.lower_numeric_for(symbol_id, start, stop, step.as_ref(), body, scope)?;
            }
            Stmt::ForIn {
                symbol_ids,
                iterators,
                body,
                ..
            } => {
                self.lower_for_in(symbol_ids, iterators, body, scope)?;
            }
        }
        Ok(())
    }

    /// `for vars in string.gmatch(s, p) do ... end`: the host holds the
    /// iteration state behind a handle; each header iteration advances it and
    /// the body reads the captures through the pm_capture_* accessors.
    fn lower_for_in_gmatch(
        &mut self,
        haystack_expr: &Expr,
        pattern_expr: &Expr,
        ids: &[SymbolId],
        body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        let captures = waluau_ast::expr_pattern_captures(pattern_expr);
        // Loop variable slots: the pattern's captures, or the whole match
        // (host capture index 0) when the pattern has none. Extra captures
        // beyond the bound variables are dropped, as in Lua.
        let slot_kinds: Vec<(waluau_ast::LuaCaptureKind, i32)> = if captures.is_empty() {
            vec![(waluau_ast::LuaCaptureKind::Str, 0)]
        } else {
            captures
                .iter()
                .enumerate()
                .map(|(index, kind)| (*kind, index as i32 + 1))
                .collect()
        };
        if ids.len() > slot_kinds.len() {
            return Err(Diagnostic::new(format!(
                "{STRING_GMATCH} for-in loop binds {} variables, but the pattern only produces {} values",
                ids.len(),
                slot_kinds.len()
            )));
        }

        let haystack = self.lower_expr(haystack_expr, scope, Some(Type::String))?;
        let pattern = self.lower_expr(pattern_expr, scope, Some(Type::String))?;
        let handle = self.emit_string_host_call(
            PM_GMATCH_HOST,
            vec![haystack, pattern],
            i32_ty.clone(),
        )?;

        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_scope = scope.fork();
        let mut phis = HashMap::new();
        for id in &mutated {
            if let Some(initial) = scope.env.get(id).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                self.function.value_symbols.insert(phi, *id);
                phis.insert(*id, phi);
            }
        }
        for (id, phi) in &phis {
            loop_scope.env.insert(*id, *phi);
        }

        let has = self.emit_string_host_call(PM_GMATCH_NEXT_HOST, vec![handle], i32_ty.clone())?;
        let zero = self.emit_i32_const(0);
        let continue_value = self.emit(Instruction::Binary {
            op: BinaryOp::NotEq,
            left: has,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        let exit_phis = self.create_exit_phis(exit, &phis);
        for (id, phi) in &phis {
            add_phi_incoming(&mut self.function, exit, exit_phis[id], (header, *phi));
        }
        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
            // The gmatch handle advance is re-emitted in the header each
            // iteration; no loop-carried values beyond the user phis.
            carries: Vec::new(),
            exit_phis: exit_phis.clone(),
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: continue_value,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let mut body_scope = loop_scope.fork();
        for (id, (kind, capture_index)) in ids.iter().zip(slot_kinds.iter()) {
            let index_value = self.emit_i32_const(*capture_index);
            let (host, value_ty) = match kind {
                waluau_ast::LuaCaptureKind::Str => (PM_CAPTURE_STRING_HOST, Type::String),
                waluau_ast::LuaCaptureKind::Position => (PM_CAPTURE_POSITION_HOST, i32_ty.clone()),
            };
            let value = self.emit_string_host_call(host, vec![index_value], value_ty.clone())?;
            self.bind_loop_var(*id, value, &value_ty, &mut body_scope.env);
            body_scope.types.insert(*id, value_ty);
        }
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_scope)?;
        }

        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for gmatch for-in loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            self.set_terminator(body_exit, Terminator::Jump(header));
            for (id, phi) in &phis {
                if let Some(next_value) = body_scope.env.get(id).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (id, _) in phis {
            let exit_phi = exit_phis[&id];
            scope.env.insert(id, exit_phi);
            self.function.value_symbols.insert(exit_phi, id);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_assert_call(
        &mut self,
        args: &[Expr],
        span: Option<waluau_ast::Span>,
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        if !(1..=2).contains(&args.len()) {
            return Err(Diagnostic::new(format!(
                "{ASSERT} expects 1 or 2 arguments, got {}",
                args.len()
            )));
        }
        let condition = self.lower_expr(&args[0], scope, Some(Type::Bool))?;
        let continue_block = self.new_block();
        let trap_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block: continue_block,
                else_block: trap_block,
            },
        );
        self.current_block = trap_block;

        let message = if let Some(message_expr) = args.get(1) {
            self.lower_expr(message_expr, scope, Some(Type::String))?
        } else {
            let assert_span = args[0].span().or(span);
            let msg_str = if let Some(sp) = assert_span {
                if let Some(source) = self.cx.sources.get(&self.file_path) {
                    let (line, expr_text) = resolve_span_to_line_and_text(source, sp);
                    format!(
                        "Assertion failed: {} at {}:{}",
                        expr_text, self.file_path, line
                    )
                } else {
                    format!("Assertion failed at {}:0", self.file_path)
                }
            } else {
                "Assertion failed".to_string()
            };
            self.emit(Instruction::String(msg_str))
        };
        self.emit(Instruction::Throw { error: message });
        self.set_terminator(trap_block, Terminator::Unreachable { span });
        self.current_block = continue_block;
        Ok(())
    }

    /// After `assert(cond)` the remaining statements only execute when `cond`
    /// held, so the full then-branch narrowing applies to the ambient scope
    /// (mirroring `assert_narrowed_scope` in waluau-hir). Where the narrowed
    /// type has a different runtime representation (pcall payload unbox, nil
    /// test) this emits the cast in the continue block and rebinds the symbol,
    /// mirroring the branch-entry materialization in `lower_if_branches`;
    /// variant narrowing is a type-level refinement only.
    fn apply_assert_narrowing(
        &mut self,
        condition: &Expr,
        scope: &mut Scope,
    ) {
        let (then_types_init, _) = self.narrowed_type_scopes(condition, &scope.types);
        for (symbol_id, narrowed_ty) in then_types_init {
            let Some(original_ty) = scope.types.get(&symbol_id).cloned() else {
                continue;
            };
            if original_ty == narrowed_ty {
                continue;
            }
            if original_ty.nullable_inner().as_ref() == Some(&narrowed_ty)
                || original_ty == Type::Unknown
            {
                let Some(original_value) = scope.env.get(&symbol_id).copied() else {
                    continue;
                };
                let narrowed_value = self.emit(Instruction::Cast {
                    value: original_value,
                    from: original_ty,
                    to: narrowed_ty.clone(),
                });
                scope.env.insert(symbol_id, narrowed_value);
                self.function
                    .value_symbols
                    .insert(narrowed_value, symbol_id);
                scope.types.insert(symbol_id, narrowed_ty);
            } else if matches!(original_ty, Type::TaggedUnion(_) | Type::TaggedVariant(_))
                && matches!(narrowed_ty, Type::TaggedUnion(_) | Type::TaggedVariant(_))
            {
                // Tagged-union values share the canonical record representation,
                // so variant narrowing needs no cast.
                scope.types.insert(symbol_id, narrowed_ty);
            }
        }
    }

    /// Recognizes `local ok, v = pcall(f, ...)` and records the discriminant
    /// link so branching on `ok` (or `assert(ok)`) narrows `v` to `f`'s return
    /// type on the success path and to `string` (the error payload) otherwise.
    /// Mirrors `pcall_discriminant_types` in waluau-hir.
    fn record_pcall_discriminant(
        &mut self,
        bindings: &[waluau_ast::Binding],
        values: &[Expr],
        types: &HashMap<SymbolId, Type>,
    ) {
        if bindings.len() != 2 {
            return;
        }
        let [Expr::Call { callee, args, .. }] = values else {
            return;
        };
        if builtin_name(callee.as_ref()).as_deref() != Some(PCALL) {
            return;
        }
        let (Some(ok_symbol), Some(payload_symbol)) =
            (bindings[0].symbol_id, bindings[1].symbol_id)
        else {
            return;
        };
        // Captured-as-cell locals live behind a mutable array cell; rebinding
        // them to an unboxed cast value would desynchronize the cell.
        if self.cell_names.contains(&ok_symbol) || self.cell_names.contains(&payload_symbol) {
            return;
        }
        let Some(first_arg) = args.first() else {
            return;
        };
        let success = match self.infer_expr_type(first_arg, types, None) {
            Ok(Type::Function { return_type, .. }) => Arc::unwrap_or_clone(return_type),
            Ok(Type::Unknown) => Type::Unknown,
            _ => return,
        };
        self.discriminants.insert(
            ok_symbol,
            DiscriminantLink {
                payload: payload_symbol,
                when_true: success,
                when_false: Type::String,
            },
        );
    }

    /// Reassigning either half of a pcall discriminant pair invalidates the
    /// correlation, so drop any link involving the symbol.
    fn sever_discriminants(&mut self, symbol_id: SymbolId) {
        self.discriminants
            .retain(|ok_symbol, link| *ok_symbol != symbol_id && link.payload != symbol_id);
    }

    /// A condition of the form `ok` or `not ok`, returning the symbol and
    /// whether the `then` branch corresponds to `ok == true`.
    fn discriminant_test_subject(condition: &Expr) -> Option<(SymbolId, bool)> {
        match condition {
            Expr::Name(_, Some(symbol_id), _) => Some((*symbol_id, true)),
            Expr::Unary {
                op: UnaryOp::Not,
                expr,
                ..
            } => match expr.as_ref() {
                Expr::Name(_, Some(symbol_id), _) => Some((*symbol_id, false)),
                _ => None,
            },
            _ => None,
        }
    }

    fn narrowed_discriminant_type_scopes(
        &self,
        condition: &Expr,
        types: &HashMap<SymbolId, Type>,
        then_types: &mut HashMap<SymbolId, Type>,
        else_types: &mut HashMap<SymbolId, Type>,
    ) {
        let Some((symbol_id, positive)) = Self::discriminant_test_subject(condition) else {
            return;
        };
        let Some(link) = self.discriminants.get(&symbol_id) else {
            return;
        };
        // Only narrow while the payload still has its original anyref-backed
        // `unknown` type; anything else means the slot no longer holds the raw
        // pcall result.
        if types.get(&link.payload) != Some(&Type::Unknown) {
            return;
        }
        let (then_ty, else_ty) = if positive {
            (&link.when_true, &link.when_false)
        } else {
            (&link.when_false, &link.when_true)
        };
        if is_narrowable_pcall_payload(then_ty) {
            then_types.insert(link.payload, then_ty.clone());
        }
        if is_narrowable_pcall_payload(else_ty) {
            else_types.insert(link.payload, else_ty.clone());
        }
    }

    /// Compute narrowed type scopes for `if result is Variant then ... else ... end`.
    /// Returns `(then_types, else_types)` — clones of `types` with the narrowed type
    /// applied to the named variable in each branch.
    fn narrowed_variant_type_scopes(
        condition: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> (HashMap<SymbolId, Type>, HashMap<SymbolId, Type>) {
        let mut then_types = types.clone();
        let mut else_types = types.clone();
        let Expr::IsVariant { expr, tag, .. } = condition else {
            return (then_types, else_types);
        };
        let Expr::Name(_, Some(symbol_id), _) = expr.as_ref() else {
            return (then_types, else_types);
        };
        let Some(ty) = types.get(symbol_id) else {
            return (then_types, else_types);
        };
        if let Some(variant) = ty.tagged_variant(tag) {
            then_types.insert(*symbol_id, Type::TaggedVariant(variant));
        }
        if let Some(remaining) = ty.remove_tagged_variant(tag) {
            else_types.insert(*symbol_id, remaining);
        }
        (then_types, else_types)
    }

    fn nil_test_subject(condition: &Expr) -> Option<(SymbolId, Option<String>, bool)> {
        // A bare `if x then` on a nullable value is a truthiness test: the
        // then-branch sees the non-nil type.
        if let Expr::Name(_, Some(symbol_id), _) = condition {
            return Some((*symbol_id, None, true));
        }
        if let Expr::Unary {
            op: UnaryOp::Not,
            expr,
            ..
        } = condition
        {
            if let Expr::Name(_, Some(symbol_id), _) = expr.as_ref() {
                return Some((*symbol_id, None, false));
            }
        }
        let Expr::Binary {
            op, left, right, ..
        } = condition
        else {
            return None;
        };
        let non_null_when_true = match op {
            BinaryOp::Eq => false,
            BinaryOp::NotEq => true,
            _ => return None,
        };
        match (left.as_ref(), right.as_ref()) {
            (Expr::Name(_, Some(symbol_id), _), Expr::Nil(..))
            | (Expr::Nil(..), Expr::Name(_, Some(symbol_id), _)) => {
                Some((*symbol_id, None, non_null_when_true))
            }
            (Expr::Field { base, name, .. }, Expr::Nil(..))
            | (Expr::Nil(..), Expr::Field { base, name, .. }) => {
                let Expr::Name(_, Some(symbol_id), _) = base.as_ref() else {
                    return None;
                };
                Some((*symbol_id, Some(name.clone()), non_null_when_true))
            }
            _ => None,
        }
    }

    fn narrow_nullable_record_field(ty: &Type, field: &str) -> Option<Type> {
        match ty {
            Type::Record(fields) => {
                let inner = fields.get(field)?.nullable_inner()?;
                let mut narrowed = fields.clone();
                Arc::make_mut(&mut narrowed).insert(field.to_string(), inner);
                Some(Type::Record(narrowed))
            }
            Type::Opaque {
                name,
                ty,
                generic_extern,
            } => Some(Type::Opaque {
                name: name.clone(),
                ty: waluau_ast::OpaquePayload::new(Self::narrow_nullable_record_field(ty, field)?),
                generic_extern: generic_extern.clone(),
            }),
            _ => None,
        }
    }

    fn narrowed_type_scopes(
        &self,
        condition: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> (HashMap<SymbolId, Type>, HashMap<SymbolId, Type>) {
        let (mut then_types, mut else_types) =
            Self::narrowed_variant_type_scopes(condition, types);
        self.narrowed_discriminant_type_scopes(condition, types, &mut then_types, &mut else_types);
        let Some((symbol_id, field, non_null_when_true)) = Self::nil_test_subject(condition) else {
            return (then_types, else_types);
        };
        let Some(ty) = types.get(&symbol_id) else {
            return (then_types, else_types);
        };
        let narrowed = if let Some(field) = field {
            let Some(narrowed) = Self::narrow_nullable_record_field(ty, &field) else {
                return (then_types, else_types);
            };
            narrowed
        } else {
            let Some(inner) = ty.nullable_inner() else {
                return (then_types, else_types);
            };
            inner
        };
        if non_null_when_true {
            then_types.insert(symbol_id, narrowed);
        } else {
            else_types.insert(symbol_id, narrowed);
        }
        (then_types, else_types)
    }

    fn lower_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        else_body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let (then_types, else_types) = self.narrowed_type_scopes(condition, &scope.types);
        let condition_value = self.lower_expr(condition, scope, Some(Type::Bool))?;
        self.lower_if_branches(
            condition_value,
            then_body,
            else_body,
            scope,
            BranchNarrowing {
                then_types,
                else_types,
            },
            None,
        )
    }

    /// Lowers the branches of an `if`, given the already-lowered boolean `condition`
    /// value and the narrowed type scopes for each branch. `pattern_binding`, when
    /// present, additionally unboxes the tagged-variant payload from `base` and binds
    /// it to `symbol_id` at the start of the `then` branch (for `if Tag(x) = expr then`).
    fn lower_if_branches(
        &mut self,
        condition: ValueId,
        then_body: &[Stmt],
        else_body: &[Stmt],
        scope: &mut Scope,
        narrowing: BranchNarrowing,
        pattern_binding: Option<(SymbolId, ValueId, Type)>,
    ) -> Result<(), Diagnostic> {
        let then_block = self.new_block();
        let else_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            },
        );

        let mut then_scope = scope.fork();
        let mut then_narrowed_values = HashMap::new();
        self.current_block = then_block;
        for (symbol_id, narrowed_ty) in narrowing.then_types {
            if let (Some(original_ty), Some(original_value)) =
                (scope.types.get(&symbol_id), then_scope.env.get(&symbol_id).copied())
            {
                if original_ty != &narrowed_ty
                    && (original_ty.nullable_inner().as_ref() == Some(&narrowed_ty)
                        || *original_ty == Type::Unknown)
                {
                    let narrowed_value = self.emit(Instruction::Cast {
                        value: original_value,
                        from: original_ty.clone(),
                        to: narrowed_ty.clone(),
                    });
                    then_scope.env.insert(symbol_id, narrowed_value);
                    then_narrowed_values.insert(symbol_id, narrowed_value);
                    self.function
                        .value_symbols
                        .insert(narrowed_value, symbol_id);
                }
            }
            then_scope.types.insert(symbol_id, narrowed_ty);
        }
        if let Some((symbol_id, base, payload_ty)) = &pattern_binding {
            let unboxed = self.unbox_tagged_variant_value(*base, payload_ty)?;
            if self.cell_names.contains(symbol_id) {
                let cell = self.emit(Instruction::ArrayNew {
                    element_ty: to_runtime_type(payload_ty),
                    elements: vec![unboxed],
                });
                then_scope.env.insert(*symbol_id, cell);
                self.function.value_symbols.insert(cell, *symbol_id);
            } else {
                then_scope.env.insert(*symbol_id, unboxed);
                self.function.value_symbols.insert(unboxed, *symbol_id);
            }
        }
        for stmt in then_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut then_scope)?;
        }
        let then_exit = self.current_block;
        if then_exit != DEAD_BLOCK {
            self.set_terminator(then_exit, Terminator::Jump(merge_block));
        }

        let mut else_scope = scope.fork();
        let mut else_narrowed_values = HashMap::new();
        self.current_block = else_block;
        for (symbol_id, narrowed_ty) in narrowing.else_types {
            if let (Some(original_ty), Some(original_value)) =
                (scope.types.get(&symbol_id), else_scope.env.get(&symbol_id).copied())
            {
                if original_ty != &narrowed_ty
                    && (original_ty.nullable_inner().as_ref() == Some(&narrowed_ty)
                        || *original_ty == Type::Unknown)
                {
                    let narrowed_value = self.emit(Instruction::Cast {
                        value: original_value,
                        from: original_ty.clone(),
                        to: narrowed_ty.clone(),
                    });
                    else_scope.env.insert(symbol_id, narrowed_value);
                    else_narrowed_values.insert(symbol_id, narrowed_value);
                    self.function
                        .value_symbols
                        .insert(narrowed_value, symbol_id);
                }
            }
            else_scope.types.insert(symbol_id, narrowed_ty);
        }
        for stmt in else_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut else_scope)?;
        }
        let else_exit = self.current_block;
        if else_exit != DEAD_BLOCK {
            self.set_terminator(else_exit, Terminator::Jump(merge_block));
        }

        if then_exit == DEAD_BLOCK && else_exit == DEAD_BLOCK {
            self.current_block = DEAD_BLOCK;
            return Ok(());
        }

        self.current_block = merge_block;
        // Sorted for deterministic phi emission order (see create_exit_phis).
        let mut merge_names: Vec<SymbolId> = scope.env.keys().cloned().collect();
        merge_names.sort_unstable();
        for name in merge_names {
            let t = then_scope.env
                .get(&name)
                .copied()
                .or_else(|| scope.env.get(&name).copied());
            let e = else_scope.env
                .get(&name)
                .copied()
                .or_else(|| scope.env.get(&name).copied());
            if let (Some(tv), Some(ev)) = (t, e) {
                if tv != ev {
                    let original = scope.env.get(&name).copied();
                    let then_is_only_narrowed =
                        then_narrowed_values.get(&name).copied() == Some(tv) && original == Some(ev);
                    let else_is_only_narrowed =
                        else_narrowed_values.get(&name).copied() == Some(ev) && original == Some(tv);
                    // Both branches narrowed (pcall discriminant: success type in
                    // one arm, error type in the other). With both exits live the
                    // branch-local casts must not merge — the ambient type stays
                    // the original storage type, so keep the original value. With
                    // one exit dead the surviving cast flows through the phi below
                    // to match the type propagated after the if.
                    let both_narrowed = then_narrowed_values.get(&name).copied() == Some(tv)
                        && else_narrowed_values.get(&name).copied() == Some(ev)
                        && then_exit != DEAD_BLOCK
                        && else_exit != DEAD_BLOCK;
                    if then_exit == DEAD_BLOCK
                        && else_narrowed_values.get(&name).copied() == Some(ev)
                    {
                        scope.env.insert(name, ev);
                        continue;
                    }
                    if else_exit == DEAD_BLOCK
                        && then_narrowed_values.get(&name).copied() == Some(tv)
                    {
                        scope.env.insert(name, tv);
                        continue;
                    }
                    if then_is_only_narrowed || else_is_only_narrowed || both_narrowed {
                        continue;
                    }
                    let mut incoming = Vec::new();
                    if then_exit != DEAD_BLOCK {
                        incoming.push((then_exit, tv));
                    }
                    if else_exit != DEAD_BLOCK {
                        incoming.push((else_exit, ev));
                    }
                    let phi = self.emit(Instruction::Phi(incoming));
                    scope.env.insert(name, phi);
                    self.function.value_symbols.insert(phi, name);
                }
            }
        }
        // Propagate narrowed types from the surviving branch when one side diverges.
        // This enables narrowing to persist after `if result is X then return ... end`.
        if then_exit == DEAD_BLOCK {
            for name in scope.types.keys().cloned().collect::<Vec<_>>() {
                if let Some(narrowed) = else_scope.types.get(&name) {
                    scope.types.insert(name, narrowed.clone());
                }
            }
        } else if else_exit == DEAD_BLOCK {
            for name in scope.types.keys().cloned().collect::<Vec<_>>() {
                if let Some(narrowed) = then_scope.types.get(&name) {
                    scope.types.insert(name, narrowed.clone());
                }
            }
        }
        Ok(())
    }

    /// Lowers the dual-purpose `if Name(binding) = expr then ... end` syntax.
    /// HIR type-checking has already validated which interpretation applies and
    /// scoped `binding` accordingly, but the IR re-derives the same dispatch
    /// (tagged-union pattern match vs. extern safe-cast) from the scrutinee's
    /// inferred type, since a single parsed `Stmt::IfCast` shape backs both:
    ///
    ///   - Tagged-union pattern match with binding (`target_name` matches a
    ///     variant tag of the scrutinee's type): emits `StructGet tag` + `Eq`
    ///     and unboxes the payload into `binding`, scoped to `then`.
    ///   - Extern safe-cast (`target_name` names an extern type): emits
    ///     `ExternCastTest` and binds the whole tested value to `binding` with
    ///     type `Type::Extern`, scoped to `then`.
    fn lower_if_cast(
        &mut self,
        parts: IfCastParts<'_>,
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let binding_symbol_id = parts
            .binding_symbol_id
            .ok_or_else(|| Diagnostic::new("if-cast binding must have a symbol ID resolved"))?;

        let scrutinee_ty = self.infer_expr_type(parts.value, &scope.types, None)?;
        if let Some(variant) = scrutinee_ty.tagged_variant(parts.target_name) {
            let payload_ty = (*variant.payload).clone();

            // Lower the scrutinee once; the underlying IR value is the canonical record.
            let base = self.lower_expr(parts.value, scope, None)?;
            let tag_id = self.variant_tag_id(parts.target_name)?;
            let tag_field_ty = Type::Numeric(NumericType::I32);
            let tag_val = self.emit(Instruction::StructGet {
                base,
                field: "tag".to_string(),
                field_ty: tag_field_ty.clone(),
            });
            let expected_tag = self.emit(Instruction::Number {
                ty: NumericType::I32,
                literal: NumberLiteral {
                    raw: tag_id.to_string(),
                },
            });
            let condition_value = self.emit(Instruction::Binary {
                op: BinaryOp::Eq,
                left: tag_val,
                right: expected_tag,
                operand_ty: tag_field_ty,
                result_ty: Type::Bool,
            });

            let mut then_types = scope.types.clone();
            then_types.insert(binding_symbol_id, payload_ty.clone());
            let else_types = scope.types.clone();
            return self.lower_if_branches(
                condition_value,
                parts.then_body,
                parts.else_body,
                scope,
                BranchNarrowing {
                    then_types,
                    else_types,
                },
                Some((binding_symbol_id, base, payload_ty)),
            );
        }

        let tested = self.lower_expr(parts.value, scope, None)?;
        let condition = self.emit(Instruction::ExternCastTest {
            value: tested,
            target_name: parts.target_name.to_string(),
        });
        let then_block = self.new_block();
        let else_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            },
        );

        let mut then_scope = scope.fork();
        then_scope.env.insert(binding_symbol_id, tested);
        then_scope.types.insert(binding_symbol_id, Type::Extern);
        self.current_block = then_block;
        for stmt in parts.then_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut then_scope)?;
        }
        let then_exit = self.current_block;
        if then_exit != DEAD_BLOCK {
            self.set_terminator(then_exit, Terminator::Jump(merge_block));
        }

        let mut else_scope = scope.fork();
        self.current_block = else_block;
        for stmt in parts.else_body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut else_scope)?;
        }
        let else_exit = self.current_block;
        if else_exit != DEAD_BLOCK {
            self.set_terminator(else_exit, Terminator::Jump(merge_block));
        }

        if then_exit == DEAD_BLOCK && else_exit == DEAD_BLOCK {
            self.current_block = DEAD_BLOCK;
            return Ok(());
        }

        self.current_block = merge_block;
        // Sorted for deterministic phi emission order (see create_exit_phis).
        let mut merge_names: Vec<SymbolId> = scope.env.keys().cloned().collect();
        merge_names.sort_unstable();
        for name in merge_names {
            let t = then_scope.env
                .get(&name)
                .copied()
                .or_else(|| scope.env.get(&name).copied());
            let e = else_scope.env
                .get(&name)
                .copied()
                .or_else(|| scope.env.get(&name).copied());
            if let (Some(tv), Some(ev)) = (t, e)
                && tv != ev
            {
                let mut incoming = Vec::new();
                if then_exit != DEAD_BLOCK {
                    incoming.push((then_exit, tv));
                }
                if else_exit != DEAD_BLOCK {
                    incoming.push((else_exit, ev));
                }
                let phi = self.emit(Instruction::Phi(incoming));
                scope.env.insert(name, phi);
                self.function.value_symbols.insert(phi, name);
            }
        }
        Ok(())
    }

    fn lower_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_scope = scope.fork();
        let mut phis = HashMap::new();
        for id in &mutated {
            if let Some(initial) = scope.env.get(id).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_scope.env.insert(*id, phi);
                self.function.value_symbols.insert(phi, *id);
                phis.insert(*id, phi);
            }
        }

        let exit_phis = self.create_exit_phis(exit, &phis);
        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
            carries: Vec::new(),
            exit_phis: exit_phis.clone(),
        });

        // Short-circuit conditions lower across several blocks; the loop
        // branch must come from wherever condition lowering ended, not
        // necessarily the header itself.
        let cond_value = self.lower_expr(condition, &loop_scope, Some(Type::Bool))?;
        let cond_exit = self.current_block;
        self.set_terminator(
            cond_exit,
            Terminator::Branch {
                condition: cond_value,
                then_block: loop_body,
                else_block: exit,
            },
        );
        for (id, phi) in &phis {
            add_phi_incoming(&mut self.function, exit, exit_phis[id], (cond_exit, *phi));
        }

        self.current_block = loop_body;
        let mut body_scope = loop_scope.fork();
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_scope)?;
        }
        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for while loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            self.set_terminator(body_exit, Terminator::Jump(header));
            for (id, phi) in &phis {
                if let Some(next_value) = body_scope.env.get(id).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (id, _) in phis {
            let exit_phi = exit_phis[&id];
            scope.env.insert(id, exit_phi);
            self.function.value_symbols.insert(exit_phi, id);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_repeat(
        &mut self,
        body: &[Stmt],
        condition: &Expr,
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let preheader = self.current_block;
        let loop_body = self.new_block();
        let check = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(loop_body));

        let mutated = collect_assigned_names(body);
        self.current_block = loop_body;
        let mut loop_scope = scope.fork();
        let mut phis = HashMap::new();
        for name in &mutated {
            if let Some(initial) = scope.env.get(name).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_scope.env.insert(*name, phi);
                self.function.value_symbols.insert(phi, *name);
                phis.insert(*name, phi);
            }
        }

        let exit_phis = self.create_exit_phis(exit, &phis);
        self.loop_stack.push(LoopContext {
            header: loop_body,
            continue_target: check,
            break_target: exit,
            phis: phis.clone(),
            carries: Vec::new(),
            exit_phis: exit_phis.clone(),
        });

        let mut body_scope = loop_scope.fork();
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_scope)?;
        }
        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for repeat-until loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit == DEAD_BLOCK {
            // The only edges into the exit block are break edges; expose the
            // exit phis they feed.
            for (name, _) in phis {
                let exit_phi = exit_phis[&name];
                scope.env.insert(name, exit_phi);
                self.function.value_symbols.insert(exit_phi, name);
            }
            self.current_block = exit;
            return Ok(());
        }
        self.set_terminator(body_exit, Terminator::Jump(check));

        self.current_block = check;
        // Short-circuit conditions lower across several blocks; branch (and
        // register phi edges) from wherever condition lowering ended.
        let cond_value = self.lower_expr(condition, &body_scope, Some(Type::Bool))?;
        let cond_exit = self.current_block;
        self.set_terminator(
            cond_exit,
            Terminator::Branch {
                condition: cond_value,
                then_block: exit,
                else_block: loop_body,
            },
        );

        for (name, phi) in &phis {
            if let Some(next_value) = body_scope.env.get(name).copied() {
                add_phi_incoming(&mut self.function, loop_body, *phi, (cond_exit, next_value));
            }
        }

        for (name, phi) in phis {
            let val = body_scope.env.get(&name).copied().unwrap_or(phi);
            let exit_phi = exit_phis[&name];
            add_phi_incoming(&mut self.function, exit, exit_phi, (cond_exit, val));
            scope.env.insert(name, exit_phi);
            self.function.value_symbols.insert(exit_phi, name);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_numeric_for(
        &mut self,
        symbol_id: &Option<SymbolId>,
        start: &Expr,
        stop: &Expr,
        step: Option<&Expr>,
        body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let symbol_id = symbol_id.expect("symbol_id should be resolved");
        let mut bounds = vec![start, stop];
        if let Some(step_expr) = step {
            bounds.push(step_expr);
        }
        let loop_ty = infer_numeric_for_loop_type(&bounds, |expr, expected| {
            self.infer_expr_type(expr, &scope.types, expected)
        })?;
        let Type::Numeric(numeric_ty) = loop_ty else {
            return Err(Diagnostic::new("numeric for-loop bounds must be numeric"));
        };
        let loop_ty = Type::Numeric(numeric_ty);
        let start_value = self.lower_expr(start, scope, Some(loop_ty.clone()))?;
        let stop_init = self.lower_expr(stop, scope, Some(loop_ty.clone()))?;
        let zero_value = self.emit(Instruction::Number {
            ty: numeric_ty,
            literal: NumberLiteral { raw: "0".into() },
        });
        let default_step_value = if step.is_none() {
            let default_step_expr = Expr::Cast {
                expr: Box::new(Expr::Number(NumberLiteral { raw: "1".into() }, None)),
                ty: loop_ty.clone(),
                span: None,
            };
            Some(self.lower_expr(&default_step_expr, scope, Some(loop_ty.clone()))?)
        } else {
            None
        };

        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_scope = scope.fork();
        let mut phis = HashMap::new();
        for id in &mutated {
            if let Some(initial) = scope.env.get(id).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_scope.env.insert(*id, phi);
                self.function.value_symbols.insert(phi, *id);
                phis.insert(*id, phi);
            }
        }
        let stop_phi = self.emit(Instruction::Phi(vec![(preheader, stop_init)]));
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, start_value)]));
        self.function.value_symbols.insert(index_phi, symbol_id);
        let step_value = if let Some(step_expr) = step {
            self.lower_expr(step_expr, &loop_scope, Some(loop_ty.clone()))?
        } else {
            default_step_value.expect("precomputed default step")
        };

        let step_positive = self.emit(Instruction::Binary {
            op: BinaryOp::Greater,
            left: step_value,
            right: zero_value,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_lt_stop = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index_phi,
            right: stop_phi,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_gt_stop = self.emit(Instruction::Binary {
            op: BinaryOp::Greater,
            left: index_phi,
            right: stop_phi,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_eq_stop_for_le = self.emit(Instruction::Binary {
            op: BinaryOp::Eq,
            left: index_phi,
            right: stop_phi,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_eq_stop_for_ge = self.emit(Instruction::Binary {
            op: BinaryOp::Eq,
            left: index_phi,
            right: stop_phi,
            operand_ty: loop_ty.clone(),
            result_ty: Type::Bool,
        });
        let i_le_stop = self.emit(Instruction::Binary {
            op: BinaryOp::Or,
            left: i_lt_stop,
            right: i_eq_stop_for_le,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let i_ge_stop = self.emit(Instruction::Binary {
            op: BinaryOp::Or,
            left: i_gt_stop,
            right: i_eq_stop_for_ge,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let forward_ok = self.emit(Instruction::Binary {
            op: BinaryOp::And,
            left: step_positive,
            right: i_le_stop,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        // Lua 5.1 / Luau semantics: any step that is not positive (including 0
        // and NaN) iterates backward, so `for i = 10, 1, 0` loops while
        // i >= stop and `for i = 1, 10, 0` iterates zero times.
        let false_value = self.emit(Instruction::Bool(false));
        let step_not_positive = self.emit(Instruction::Binary {
            op: BinaryOp::Eq,
            left: step_positive,
            right: false_value,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let backward_ok = self.emit(Instruction::Binary {
            op: BinaryOp::And,
            left: step_not_positive,
            right: i_ge_stop,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let loop_cond = self.emit(Instruction::Binary {
            op: BinaryOp::Or,
            left: forward_ok,
            right: backward_ok,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });

        let exit_phis = self.create_exit_phis(exit, &phis);
        for (id, phi) in &phis {
            add_phi_incoming(&mut self.function, exit, exit_phis[id], (header, *phi));
        }
        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
            carries: vec![
                LoopCarry {
                    phi: stop_phi,
                    update: CarryUpdate::Invariant(stop_phi),
                },
                LoopCarry {
                    phi: index_phi,
                    update: CarryUpdate::Increment {
                        base: index_phi,
                        step: step_value,
                        ty: loop_ty.clone(),
                    },
                },
            ],
            exit_phis: exit_phis.clone(),
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: loop_cond,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let mut body_scope = loop_scope.fork();
        self.bind_loop_var(symbol_id, index_phi, &loop_ty, &mut body_scope.env);
        body_scope.types.insert(symbol_id, loop_ty.clone());
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_scope)?;
        }
        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for numeric for loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            let stop_next = self.emit(Instruction::Binary {
                op: BinaryOp::Add,
                left: stop_phi,
                right: zero_value,
                operand_ty: loop_ty.clone(),
                result_ty: loop_ty.clone(),
            });
            let next_index = self.emit(Instruction::Binary {
                op: BinaryOp::Add,
                left: index_phi,
                right: step_value,
                operand_ty: loop_ty.clone(),
                result_ty: loop_ty.clone(),
            });
            self.set_terminator(body_exit, Terminator::Jump(header));
            add_phi_incoming(&mut self.function, header, stop_phi, (body_exit, stop_next));
            add_phi_incoming(
                &mut self.function,
                header,
                index_phi,
                (body_exit, next_index),
            );
            for (id, phi) in &phis {
                if let Some(next_value) = body_scope.env.get(id).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (id, _) in phis {
            let exit_phi = exit_phis[&id];
            scope.env.insert(id, exit_phi);
            self.function.value_symbols.insert(exit_phi, id);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_for_in(
        &mut self,
        symbol_ids: &Option<Vec<SymbolId>>,
        iterators: &[Expr],
        body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let ids = symbol_ids.as_ref().expect("symbol_ids should be resolved");
        if let [iterator] = iterators {
            if let Some((haystack_expr, pattern_expr)) = gmatch_iterator_args(iterator) {
                return self
                    .lower_for_in_gmatch(haystack_expr, pattern_expr, ids, body, scope);
            }
            if let Some(array_expr) = waluau_ast::builtin_ipairs_call_arg(iterator) {
                let array_ty = first_of_multi(self.infer_expr_type(
                    array_expr,
                    &scope.types,
                    None,
                )?);
                let Type::Array(element_ty) = &array_ty else {
                    return Err(Diagnostic::new("ipairs(...) requires an array"));
                };
                if ids.len() != 1 && ids.len() != 2 {
                    return Err(Diagnostic::new(format!(
                        "ipairs for-in loop expects 1 or 2 loop variables, got {}",
                        ids.len()
                    )));
                }
                let array_val = self.lower_expr(array_expr, scope, Some(array_ty.clone()))?;
                return self.lower_indexed_for_in(
                    array_val,
                    element_ty,
                    IndexedLoopBindings::ArrayNext,
                    ids,
                    body,
                    scope,
                );
            }
            if let Some(record_expr) = waluau_ast::pairs_call_arg(iterator) {
                return self.lower_for_in_record_pairs(record_expr, ids, body, scope);
            }
            let iterator_ty = self.infer_expr_type(iterator, &scope.types, None)?;
            if let Type::Multi(slots) = &iterator_ty {
                // One expression producing `(iterator, state[, control])`:
                // the value form of the generic-for protocol.
                return self.lower_for_in_protocol(
                    ProtocolSource::MultiValue {
                        expr: iterator,
                        slots: slots.clone(),
                    },
                    ids,
                    body,
                    scope,
                );
            }
            if let Type::Array(element_ty) = &iterator_ty {
                if ids.len() != 1 && ids.len() != 2 {
                    return Err(Diagnostic::new(format!(
                        "array for-in loop expects 1 or 2 loop variables, got {}",
                        ids.len()
                    )));
                }
                let array_val = self.lower_expr(iterator, scope, Some(iterator_ty.clone()))?;
                return self.lower_indexed_for_in(
                    array_val,
                    element_ty,
                    IndexedLoopBindings::Array,
                    ids,
                    body,
                    scope,
                );
            }

            let Type::Function {
                params,
                return_type,
                ..
            } = iterator_ty
            else {
                return Err(Diagnostic::new("for-in iterator must be a function"));
            };
            if !params.is_empty() {
                return Err(Diagnostic::new(
                    "for-in iterator function must not require parameters",
                ));
            }
            return self.lower_for_in_function(Arc::unwrap_or_clone(return_type), ids, iterator, body, scope);
        }
        // `for k[, v] in next, t[, nil]`: the builtin `next` iterates a
        // record's fields like `pairs`, or an array with the index bound
        // first (`next` yields keys first, unlike a plain array loop).
        if let Some(target) = waluau_ast::for_in_builtin_next_target(iterators) {
            let target = target?;
            let target_ty = self.infer_expr_type(target, &scope.types, None)?;
            if let Type::Array(element_ty) = &target_ty {
                if ids.len() != 1 && ids.len() != 2 {
                    return Err(Diagnostic::new(format!(
                        "next for-in loop expects 1 or 2 loop variables, got {}",
                        ids.len()
                    )));
                }
                let array_val = self.lower_expr(target, scope, Some(target_ty.clone()))?;
                return self.lower_indexed_for_in(
                    array_val,
                    element_ty,
                    IndexedLoopBindings::ArrayNext,
                    ids,
                    body,
                    scope,
                );
            }
            return self.lower_for_in_record_pairs(target, ids, body, scope);
        }
        self.lower_for_in_protocol(
            ProtocolSource::ExprList {
                function: &iterators[0],
                state: &iterators[1],
                control: iterators.get(2),
            },
            ids,
            body,
            scope,
        )
    }

    /// `for k, v... in f, s[, c0]` — the Lua generic-for protocol (PIL 7.3):
    /// each iteration calls `f(s, control)`; a nil first result ends the
    /// loop, otherwise it becomes the first loop variable and feeds back as
    /// the next control value. The state is evaluated once; the control is a
    /// loop-carried phi starting at `c0` (nil when omitted).
    fn lower_for_in_protocol(
        &mut self,
        source: ProtocolSource<'_>,
        ids: &[SymbolId],
        body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let (f_ty, f_val, state_val, control_start) = match source {
            ProtocolSource::ExprList {
                function,
                state,
                control,
            } => {
                let f_ty = self.infer_expr_type(function, &scope.types, None)?;
                let Some(protocol) = waluau_ast::iterator_protocol(&f_ty) else {
                    return Err(Diagnostic::new(
                        "for-in iterator list must start with an iterator function (state, control) -> (control?, values...)",
                    ));
                };
                let f_val = self.lower_expr(function, scope, Some(f_ty.clone()))?;
                let state_val =
                    self.lower_expr(state, scope, Some(protocol.state_ty.clone()))?;
                let control_start = match control {
                    Some(expr) => {
                        self.lower_expr(expr, scope, Some(protocol.control_param_ty.clone()))?
                    }
                    None => self.emit(Instruction::Null {
                        ty: protocol.control_param_ty.clone(),
                    }),
                };
                (f_ty, f_val, state_val, control_start)
            }
            ProtocolSource::MultiValue { expr, slots } => {
                let Some(protocol) = slots.first().and_then(waluau_ast::iterator_protocol)
                else {
                    return Err(Diagnostic::new(
                        "for-in over a multi-value iterator expects (iterator, state[, control])",
                    ));
                };
                let multi_val = self.lower_expr(expr, scope, None)?;
                let f_val = self.emit(Instruction::MultiGet {
                    value: multi_val,
                    index: 0,
                    ty: slots[0].clone(),
                });
                let state_raw = self.emit(Instruction::MultiGet {
                    value: multi_val,
                    index: 1,
                    ty: slots[1].clone(),
                });
                let state_val = self.coerce_value(
                    state_raw,
                    slots[1].clone(),
                    Some(protocol.state_ty.clone()),
                )?;
                let control_start = if let Some(control_slot) = slots.get(2) {
                    let raw = self.emit(Instruction::MultiGet {
                        value: multi_val,
                        index: 2,
                        ty: control_slot.clone(),
                    });
                    self.coerce_value(
                        raw,
                        control_slot.clone(),
                        Some(protocol.control_param_ty.clone()),
                    )?
                } else {
                    self.emit(Instruction::Null {
                        ty: protocol.control_param_ty.clone(),
                    })
                };
                (slots[0].clone(), f_val, state_val, control_start)
            }
        };
        let protocol =
            waluau_ast::iterator_protocol(&f_ty).expect("protocol shape validated above");
        let multi_return = protocol.return_slots.len() > 1;
        let declared_return_ty = if multi_return {
            Type::Multi(protocol.return_slots.clone())
        } else {
            protocol.return_slots[0].clone()
        };
        let first_slot_ty = protocol.return_slots[0].clone();

        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_scope = scope.fork();
        let mut phis = HashMap::new();
        for id in &mutated {
            if let Some(initial) = scope.env.get(id).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_scope.env.insert(*id, phi);
                self.function.value_symbols.insert(phi, *id);
                phis.insert(*id, phi);
            }
        }
        let control_phi = self.emit(Instruction::Phi(vec![(preheader, control_start)]));

        let call = self.emit(Instruction::CallValue {
            callee: f_val,
            args: vec![state_val, control_phi],
            params: vec![
                protocol.state_ty.clone(),
                protocol.control_param_ty.clone(),
            ],
            return_type: declared_return_ty,
        });
        let next_control_opt = if multi_return {
            self.emit(Instruction::MultiGet {
                value: call,
                index: 0,
                ty: first_slot_ty.clone(),
            })
        } else {
            call
        };
        let is_null = self.emit(Instruction::IsNull {
            value: next_control_opt,
            ty: protocol.control_ty.clone(),
        });
        let exit_phis = self.create_exit_phis(exit, &phis);
        for (id, phi) in &phis {
            add_phi_incoming(&mut self.function, exit, exit_phis[id], (header, *phi));
        }
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: is_null,
                then_block: exit,
                else_block: loop_body,
            },
        );

        self.current_block = loop_body;
        let mut body_scope = loop_scope.fork();
        // Narrow the control result and compute the next control value before
        // any user statement runs, so continue edges can reuse it.
        let control_val = self.emit(Instruction::Cast {
            value: next_control_opt,
            from: first_slot_ty,
            to: protocol.control_ty.clone(),
        });
        let next_control = self.coerce_value(
            control_val,
            protocol.control_ty.clone(),
            Some(protocol.control_param_ty.clone()),
        )?;
        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
            carries: vec![LoopCarry {
                phi: control_phi,
                update: CarryUpdate::Invariant(next_control),
            }],
            exit_phis: exit_phis.clone(),
        });

        self.bind_loop_var(ids[0], control_val, &protocol.control_ty, &mut body_scope.env);
        body_scope.types.insert(ids[0], protocol.control_ty.clone());
        for (index, id) in ids.iter().enumerate().skip(1) {
            let slot_ty = protocol.return_slots[index].clone();
            let value = self.emit(Instruction::MultiGet {
                value: call,
                index,
                ty: slot_ty.clone(),
            });
            self.bind_loop_var(*id, value, &slot_ty, &mut body_scope.env);
            body_scope.types.insert(*id, slot_ty);
        }

        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_scope)?;
        }

        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for protocol for-in loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            self.set_terminator(body_exit, Terminator::Jump(header));
            add_phi_incoming(
                &mut self.function,
                header,
                control_phi,
                (body_exit, next_control),
            );
            for (id, phi) in &phis {
                if let Some(next_value) = body_scope.env.get(id).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (id, _) in phis {
            let exit_phi = exit_phis[&id];
            scope.env.insert(id, exit_phi);
            self.function.value_symbols.insert(exit_phi, id);
        }
        self.current_block = exit;
        Ok(())
    }

    /// `for name, value in pairs(record_value)`: iterates the record's fields
    /// in field order (alphabetical). Field names are compile-time constants,
    /// so the loop reads them from a string array built here, with the field
    /// values snapshot into a parallel array read by the same loop index. The
    /// type checker guarantees every field shares one type before this runs.
    fn lower_for_in_record_pairs(
        &mut self,
        record_expr: &Expr,
        ids: &[SymbolId],
        body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        if ids.is_empty() || ids.len() > 2 {
            return Err(Diagnostic::new(format!(
                "pairs for-in loop expects 1 or 2 loop variables, got {}",
                ids.len()
            )));
        }
        let record_ty = self.infer_expr_type(record_expr, &scope.types, None)?;
        let Some(fields) = waluau_ast::pairs_record_fields(&record_ty).cloned() else {
            return Err(Diagnostic::new(
                "pairs(...) requires an enum type or a record value",
            ));
        };
        let record_val = self.lower_expr(record_expr, scope, Some(record_ty.clone()))?;
        let name_vals: Vec<ValueId> = fields
            .keys()
            .map(|name| self.emit(Instruction::String(name.clone())))
            .collect();
        let names_array = self.emit(Instruction::ArrayNew {
            element_ty: Type::String,
            elements: name_vals,
        });
        let values = if ids.len() == 2 {
            let value_ty = fields
                .values()
                .next()
                .expect("the type checker rejects binding a value from an empty record")
                .clone();
            // A local's storage record can differ from its checked type (e.g.
            // a nullable slot narrowed to its inner type); read through the
            // storage type and cast back, mirroring plain field access.
            let storage_fields = match record_expr {
                Expr::Name(_, Some(symbol_id), _) => {
                    self.symbol_storage_types.get(symbol_id).cloned()
                }
                _ => None,
            };
            let mut value_vals = Vec::with_capacity(fields.len());
            for (name, field_ty) in &fields {
                let storage_field_ty = storage_fields
                    .as_ref()
                    .and_then(|ty| ty.record_field(name))
                    .unwrap_or_else(|| field_ty.clone());
                let value = self.emit(Instruction::StructGet {
                    base: record_val,
                    field: name.clone(),
                    field_ty: storage_field_ty.clone(),
                });
                let value = if storage_field_ty.nullable_inner().as_ref() == Some(field_ty) {
                    self.emit(Instruction::Cast {
                        value,
                        from: storage_field_ty,
                        to: field_ty.clone(),
                    })
                } else {
                    self.coerce_value(value, storage_field_ty, Some(field_ty.clone()))?
                };
                value_vals.push(value);
            }
            let values_array = self.emit(Instruction::ArrayNew {
                element_ty: value_ty.clone(),
                elements: value_vals,
            });
            Some((values_array, value_ty))
        } else {
            None
        };
        self.lower_indexed_for_in(
            names_array,
            &Type::String,
            IndexedLoopBindings::RecordPairs { values },
            ids,
            body,
            scope,
        )
    }

    /// The shared index-driven loop behind array for-in and record `pairs`:
    /// walks `array_val` from 0 to its length, binding loop variables per
    /// `bindings` for each element.
    fn lower_indexed_for_in(
        &mut self,
        array_val: ValueId,
        element_ty: &Type,
        bindings: IndexedLoopBindings,
        ids: &[SymbolId],
        body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        {
            let array_len_init = self.emit(Instruction::ArrayLen { array: array_val });
            let const_zero = self.emit(Instruction::Number {
                ty: NumericType::I32,
                literal: NumberLiteral { raw: "0".into() },
            });
            let const_one = self.emit(Instruction::Number {
                ty: NumericType::I32,
                literal: NumberLiteral { raw: "1".into() },
            });

            let preheader = self.current_block;
            let header = self.new_block();
            let loop_body = self.new_block();
            let exit = self.new_block();
            self.set_terminator(preheader, Terminator::Jump(header));

            let mutated = collect_assigned_names(body);
            self.current_block = header;
            let mut loop_scope = scope.fork();
            let mut phis = HashMap::new();
            for id in &mutated {
                if let Some(initial) = scope.env.get(id).copied() {
                    let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                    loop_scope.env.insert(*id, phi);
                    self.function.value_symbols.insert(phi, *id);
                    phis.insert(*id, phi);
                }
            }
            let array_len_phi = self.emit(Instruction::Phi(vec![(preheader, array_len_init)]));
            let index_phi = self.emit(Instruction::Phi(vec![(preheader, const_zero)]));

            let loop_cond = self.emit(Instruction::Binary {
                op: BinaryOp::Less,
                left: index_phi,
                right: array_len_phi,
                operand_ty: Type::Numeric(NumericType::I32),
                result_ty: Type::Bool,
            });
            let exit_phis = self.create_exit_phis(exit, &phis);
            for (id, phi) in &phis {
                add_phi_incoming(&mut self.function, exit, exit_phis[id], (header, *phi));
            }
            self.loop_stack.push(LoopContext {
                header,
                continue_target: header,
                break_target: exit,
                phis: phis.clone(),
                carries: vec![
                    LoopCarry {
                        phi: array_len_phi,
                        update: CarryUpdate::Invariant(array_len_phi),
                    },
                    LoopCarry {
                        phi: index_phi,
                        update: CarryUpdate::Increment {
                            base: index_phi,
                            step: const_one,
                            ty: Type::Numeric(NumericType::I32),
                        },
                    },
                ],
                exit_phis: exit_phis.clone(),
            });
            self.set_terminator(
                header,
                Terminator::Branch {
                    condition: loop_cond,
                    then_block: loop_body,
                    else_block: exit,
                },
            );

            self.current_block = loop_body;
            let mut body_scope = loop_scope.fork();

            let element_val = self.emit(Instruction::ArrayGet {
                array: array_val,
                index: index_phi,
                element_ty: element_ty.clone(),
            });

            match &bindings {
                IndexedLoopBindings::Array => {
                    if ids.len() == 1 {
                        self.bind_loop_var(ids[0], element_val, element_ty, &mut body_scope.env);
                        body_scope.types.insert(ids[0], element_ty.clone());
                    } else {
                        let visible_index = self.emit(Instruction::Binary {
                            op: BinaryOp::Add,
                            left: index_phi,
                            right: const_one,
                            operand_ty: Type::Numeric(NumericType::I32),
                            result_ty: Type::Numeric(NumericType::I32),
                        });
                        self.bind_loop_var(
                            ids[0],
                            visible_index,
                            &Type::Numeric(NumericType::I32),
                            &mut body_scope.env,
                        );
                        body_scope.types.insert(ids[0], Type::Numeric(NumericType::I32));
                        self.bind_loop_var(ids[1], element_val, element_ty, &mut body_scope.env);
                        body_scope.types.insert(ids[1], element_ty.clone());
                    }
                }
                IndexedLoopBindings::ArrayNext => {
                    let visible_index = self.emit(Instruction::Binary {
                        op: BinaryOp::Add,
                        left: index_phi,
                        right: const_one,
                        operand_ty: Type::Numeric(NumericType::I32),
                        result_ty: Type::Numeric(NumericType::I32),
                    });
                    self.bind_loop_var(
                        ids[0],
                        visible_index,
                        &Type::Numeric(NumericType::I32),
                        &mut body_scope.env,
                    );
                    body_scope.types.insert(ids[0], Type::Numeric(NumericType::I32));
                    if let Some(id) = ids.get(1) {
                        self.bind_loop_var(*id, element_val, element_ty, &mut body_scope.env);
                        body_scope.types.insert(*id, element_ty.clone());
                    }
                }
                IndexedLoopBindings::RecordPairs { values } => {
                    self.bind_loop_var(ids[0], element_val, element_ty, &mut body_scope.env);
                    body_scope.types.insert(ids[0], element_ty.clone());
                    if let Some((values_array, value_ty)) = values {
                        let value_val = self.emit(Instruction::ArrayGet {
                            array: *values_array,
                            index: index_phi,
                            element_ty: value_ty.clone(),
                        });
                        self.bind_loop_var(ids[1], value_val, value_ty, &mut body_scope.env);
                        body_scope.types.insert(ids[1], value_ty.clone());
                    }
                }
            }

            for stmt in body {
                if self.current_block == DEAD_BLOCK {
                    break;
                }
                self.lower_stmt(stmt, &mut body_scope)?;
            }

            let loop_ctx = self
                .loop_stack
                .pop()
                .expect("loop stack must contain entry for array for loop");
            let phis = loop_ctx.phis;
            let body_exit = self.current_block;
            if body_exit != DEAD_BLOCK {
                let array_len_next = self.emit(Instruction::Binary {
                    op: BinaryOp::Add,
                    left: array_len_phi,
                    right: const_zero,
                    operand_ty: Type::Numeric(NumericType::I32),
                    result_ty: Type::Numeric(NumericType::I32),
                });
                let next_index = self.emit(Instruction::Binary {
                    op: BinaryOp::Add,
                    left: index_phi,
                    right: const_one,
                    operand_ty: Type::Numeric(NumericType::I32),
                    result_ty: Type::Numeric(NumericType::I32),
                });
                self.set_terminator(body_exit, Terminator::Jump(header));
                add_phi_incoming(
                    &mut self.function,
                    header,
                    array_len_phi,
                    (body_exit, array_len_next),
                );
                add_phi_incoming(
                    &mut self.function,
                    header,
                    index_phi,
                    (body_exit, next_index),
                );
                for (name, phi) in &phis {
                    if let Some(next_value) = body_scope.env.get(name).copied() {
                        add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                    }
                }
            }

            for (name, _) in phis {
                let exit_phi = exit_phis[&name];
                scope.env.insert(name, exit_phi);
                self.function.value_symbols.insert(exit_phi, name);
            }
            self.current_block = exit;
            Ok(())
        }
    }

    /// `for ... in iter` over a parameterless closure: the iterator is called
    /// once per iteration, continuing while its first return value is true.
    fn lower_for_in_function(
        &mut self,
        return_type: Type,
        ids: &[SymbolId],
        iterator: &Expr,
        body: &[Stmt],
        scope: &mut Scope,
    ) -> Result<(), Diagnostic> {
        let return_values = match return_type {
            Type::Multi(values) => values,
            other => vec![other],
        };
        if return_values.len() != ids.len() + 1 {
            return Err(Diagnostic::new(format!(
                "for-in iterator expects {} return values (bool + {} loop values), got {}",
                ids.len() + 1,
                ids.len(),
                return_values.len()
            )));
        }
        if return_values[0] != Type::Bool {
            return Err(Diagnostic::new(
                "for-in iterator first return value must be bool",
            ));
        }
        let loop_value_types = return_values.into_iter().skip(1).collect::<Vec<_>>();
        let return_ty = Type::Multi(
            std::iter::once(Type::Bool)
                .chain(loop_value_types.clone())
                .collect(),
        );
        let direct_iterator_name = match iterator {
            Expr::Name(name, Some(symbol_id), _) => self.cx.signatures.get(symbol_id).and_then(|(params, ret)| {
                if params.is_empty() && *ret == return_ty {
                    Some(name.clone())
                } else {
                    None
                }
            }),
            _ => None,
        };
        let iterator_value = if direct_iterator_name.is_none() {
            Some(self.lower_expr(iterator, scope, None)?)
        } else {
            None
        };

        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        let mutated = collect_assigned_names(body);
        self.current_block = header;
        let mut loop_scope = scope.fork();
        let mut phis = HashMap::new();
        for id in &mutated {
            if let Some(initial) = scope.env.get(id).copied() {
                let phi = self.emit(Instruction::Phi(vec![(preheader, initial)]));
                loop_scope.env.insert(*id, phi);
                self.function.value_symbols.insert(phi, *id);
                phis.insert(*id, phi);
            }
        }

        let call = if let Some(name) = direct_iterator_name {
            self.emit(Instruction::Call {
                name,
                symbol_id: None,
                args: Vec::new(),
            })
        } else {
            self.emit(Instruction::CallValue {
                callee: iterator_value.expect("lowered above when not direct"),
                args: Vec::new(),
                params: Vec::new(),
                return_type: return_ty,
            })
        };
        let continue_value = self.emit(Instruction::MultiGet {
            value: call,
            index: 0,
            ty: Type::Bool,
        });
        let exit_phis = self.create_exit_phis(exit, &phis);
        for (id, phi) in &phis {
            add_phi_incoming(&mut self.function, exit, exit_phis[id], (header, *phi));
        }
        self.loop_stack.push(LoopContext {
            header,
            continue_target: header,
            break_target: exit,
            phis: phis.clone(),
            // The iterator call is re-emitted in the header each iteration, so
            // there are no implicit loop-carried phis beyond the user phis.
            carries: Vec::new(),
            exit_phis: exit_phis.clone(),
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: continue_value,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let mut body_scope = loop_scope.fork();
        for (index, (id, ty)) in ids.iter().zip(loop_value_types.iter()).enumerate() {
            let value = self.emit(Instruction::MultiGet {
                value: call,
                index: index + 1,
                ty: ty.clone(),
            });
            self.bind_loop_var(*id, value, ty, &mut body_scope.env);
            body_scope.types.insert(*id, ty.clone());
        }
        for stmt in body {
            if self.current_block == DEAD_BLOCK {
                break;
            }
            self.lower_stmt(stmt, &mut body_scope)?;
        }

        let loop_ctx = self
            .loop_stack
            .pop()
            .expect("loop stack must contain entry for for-in loop");
        let phis = loop_ctx.phis;
        let body_exit = self.current_block;
        if body_exit != DEAD_BLOCK {
            self.set_terminator(body_exit, Terminator::Jump(header));
            for (id, phi) in &phis {
                if let Some(next_value) = body_scope.env.get(id).copied() {
                    add_phi_incoming(&mut self.function, header, *phi, (body_exit, next_value));
                }
            }
        }

        for (id, _) in phis {
            let exit_phi = exit_phis[&id];
            scope.env.insert(id, exit_phi);
            self.function.value_symbols.insert(exit_phi, id);
        }
        self.current_block = exit;
        Ok(())
    }

    fn lower_expr(
        &mut self,
        expr: &Expr,
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let previous = std::mem::replace(&mut self.current_span, expr.span());
        let result = self.lower_expr_inner(expr, scope, expected);
        self.current_span = previous;
        result
    }

    fn lower_expr_inner(
        &mut self,
        expr: &Expr,
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let value = match expr {
            Expr::Number(number, _) => {
                let ty = match self.infer_expr_type(expr, &scope.types, expected)? {
                    Type::Numeric(ty) => ty,
                    Type::Bool => unreachable!("number literal cannot lower as bool"),
                    Type::Unit => {
                        return Err(Diagnostic::new("numeric literal is not assignable to unit"));
                    }
                    Type::String => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to string",
                        ));
                    }
                    Type::Bytes => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to bytes",
                        ));
                    }
                    Type::Buffer => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to buffer",
                        ));
                    }
                    Type::Extern | Type::ExternSubtype(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to extern",
                        ));
                    }
                    Type::Nil => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to nil",
                        ));
                    }
                    nullable @ Type::Nullable(_) => {
                        // A literal in `i32?` position lowers at the inner
                        // numeric type and boxes into the nullable.
                        let Some(Type::Numeric(numeric)) = nullable.nullable_inner() else {
                            return Err(Diagnostic::new(format!(
                                "numeric literal is not assignable to {nullable}",
                            )));
                        };
                        let value = self.emit(Instruction::Number {
                            ty: numeric,
                            literal: number.clone(),
                        });
                        return self.coerce_value(
                            value,
                            Type::Numeric(numeric),
                            Some(nullable),
                        );
                    }
                    Type::Named { name, .. } => {
                        return Err(Diagnostic::new(format!(
                            "numeric literal is not assignable to {name}",
                        )));
                    }
                    Type::Opaque { name, .. } => {
                        return Err(Diagnostic::new(format!(
                            "numeric literal is not assignable to {name}",
                        )));
                    }
                    Type::Array(_) | Type::Variadic(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to array",
                        ));
                    }
                    Type::TypedArray(kind) => {
                        return Err(Diagnostic::new(format!(
                            "numeric literal is not assignable to {}",
                            kind.type_name(),
                        )));
                    }
                    Type::Function { .. } => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to function",
                        ));
                    }
                    Type::Multi(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to multiple values",
                        ));
                    }
                    Type::Record(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to namespace",
                        ));
                    }
                    Type::TypeParam(_) => {
                        return Err(Diagnostic::new(
                            "generic type parameters must be specialized before IR lowering",
                        ));
                    }
                    Type::Thread => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to thread",
                        ));
                    }
                    Type::TaggedVariant(_) | Type::TaggedUnion(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to tagged union type",
                        ));
                    }
                    Type::StringLiteralUnion(_) => {
                        return Err(Diagnostic::new(
                            "numeric literal is not assignable to string",
                        ));
                    }
                    Type::Unknown => {
                        // Boxing a bare literal into `unknown`: lower it at its
                        // default numeric type, then box the result into anyref.
                        let ty = match self.infer_expr_type(expr, &scope.types, None)? {
                            Type::Numeric(ty) => ty,
                            other => {
                                return Err(Diagnostic::new(format!(
                                    "cannot box numeric literal as unknown: {other}"
                                )));
                            }
                        };
                        let value = self.emit(Instruction::Number {
                            ty,
                            literal: number.clone(),
                        });
                        return self.coerce_value(value, Type::Numeric(ty), Some(Type::Unknown));
                    }
                };
                self.emit(Instruction::Number {
                    ty,
                    literal: number.clone(),
                })
            }
            Expr::Bool(value, _) => {
                let value = self.emit(Instruction::Bool(*value));
                self.coerce_value(value, Type::Bool, expected)?
            }
            Expr::Nil(_) => {
                let ty = match expected.clone() {
                    Some(nullable @ Type::Nullable(_)) if nullable.is_boxed_nullable() => nullable,
                    Some(Type::Nullable(inner)) => Arc::unwrap_or_clone(inner),
                    Some(Type::Extern) => Type::Extern,
                    // nil as an `unknown` value is a null anyref (e.g. nil
                    // passed through varargs or compared dynamically).
                    Some(Type::Unknown) => Type::Unknown,
                    Some(Type::Nil) => Type::Nil,
                    Some(other) => {
                        return Err(Diagnostic::new(format!(
                            "nil is only assignable to nullable &scope.types, got {other}"
                        )));
                    }
                    None => Type::Extern,
                };
                let value = self.emit(Instruction::Null { ty: ty.clone() });
                self.coerce_value(value, ty, expected)?
            }
            Expr::String(value, _) => {
                let value = self.emit(Instruction::String(value.clone()));
                self.coerce_value(value, Type::String, expected)?
            }
            Expr::Bytes(value, _) => {
                let value = self.emit(Instruction::Bytes(value.clone()));
                self.coerce_value(value, Type::Bytes, expected)?
            }
            Expr::Vararg(_) => {
                let value = self
                    .vararg_value
                    .ok_or_else(|| Diagnostic::new("'...' used outside a vararg function"))?;
                match expected {
                    // Pack context: hand the pack over unchanged. Any element
                    // annotation is static-only, so the expectation's element
                    // never rewrites the boxed runtime pack.
                    Some(Type::Variadic(_)) => value,
                    other => {
                        self.coerce_value(value, Type::Variadic(Arc::new(Type::Unknown)), other)?
                    }
                }
            }
            Expr::Name(name, symbol_id, _) => {
                let symbol_id = symbol_id.expect("symbol_id should be resolved");
                if let Some(value) = scope.env.get(&symbol_id).copied() {
                    let actual = scope.types.get(&symbol_id).cloned().ok_or_else(|| {
                        Diagnostic::new(format!("unknown local '{name}' during IR lowering"))
                    })?;
                    if self.cell_names.contains(&symbol_id) {
                        let index0 = self.emit(Instruction::Number {
                            ty: NumericType::I32,
                            literal: NumberLiteral { raw: "0".into() },
                        });
                        let val = self.emit(Instruction::ArrayGet {
                            array: value,
                            index: index0,
                            element_ty: to_runtime_type(&actual),
                        });
                        self.coerce_value(val, actual, expected)?
                    } else {
                        self.coerce_value(value, actual, expected)?
                    }
                } else if let Some((global, actual)) = self.cx.globals.get(&symbol_id).cloned() {
                    let value = self.emit(Instruction::GlobalGet {
                        global,
                        ty: to_runtime_type(&actual),
                    });
                    self.coerce_value(value, actual, expected)?
                } else if let Some(signature) =
                    self.intrinsic_value_signature_from_expected(name, expected.as_ref())
                {
                    let (params, return_type) = signature?;
                    self.lower_intrinsic_value(name, params, return_type, expected)?
                } else if let Some((params, return_type)) =
                    self.cx.host_import_signatures.get(&symbol_id).cloned()
                {
                    self.lower_host_import_value(
                        name,
                        symbol_id,
                        params,
                        return_type,
                        expected,
                    )?
                } else if let Some((params, return_type)) = self.cx.signatures.get(&symbol_id).cloned() {
                    let value = self.emit(Instruction::Closure {
                        name: name.clone(),
                        captures: Vec::new(),
                        params: params.clone(),
                        return_type: return_type.clone(),
                    });
                    let actual = Type::Function {
                        params,
                        return_type: Arc::new(return_type),
                        has_self: false,
                    };
                    self.coerce_value(value, actual, expected)?
                } else if let Some((params, return_type)) =
                    waluau_hir::intrinsic_values::unique_intrinsic_value_signature(name)
                {
                    // A bare intrinsic with a single value signature
                    // (`print`, `tostring`) needs no annotation.
                    self.lower_intrinsic_value(name, params, return_type, expected)?
                } else {
                    return Err(Diagnostic::new(format!(
                        "unknown local '{name}' during IR lowering"
                    )));
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                resolved_name,
                args,
                ..
            } => {
                let receiver_ty = self.infer_expr_type(receiver, &scope.types, None)?;
                if receiver_ty == Type::String {
                    let mut call_args = Vec::with_capacity(args.len() + 1);
                    call_args.push((**receiver).clone());
                    call_args.extend_from_slice(args);
                    let builtin = match name.as_str() {
                        "find" => Some(STRING_FIND),
                        "match" => Some(STRING_MATCH),
                        "gmatch" => Some(STRING_GMATCH),
                        "gsub" => Some(STRING_GSUB),
                        "len" => Some(STRING_LEN),
                        "sub" => Some(STRING_SUB),
                        "rep" => Some(STRING_REP),
                        "byte" => Some(STRING_BYTE),
                        "upper" => Some(STRING_UPPER),
                        "lower" => Some(STRING_LOWER),
                        "format" => Some(STRING_FORMAT),
                        "reverse" => Some(STRING_REVERSE),
                        "split" => Some(STRING_SPLIT),
                        _ => None,
                    };
                    if let Some(builtin) = builtin {
                    if let Some(result) = self.lower_string_builtin_call(
                        builtin,
                        &call_args,
                        scope,
                        expected.clone(),
                    ) {
                        return result;
                    }
                    }
                }
                if let Some(result) = self.lower_promise_await_method_call(
                    receiver,
                    name,
                    args,
                    scope,
                    expected.clone(),
                ) {
                    return result;
                }
                let type_method =
                    type_method_signature(
                        &receiver_ty,
                        resolved_name.as_deref(),
                        name,
                        self.cx.field_call_signatures,
                    );
                // HIR resolves the nominal target before opaque aliases are
                // erased. A guarded recursive field is `unknown` at the
                // finite runtime edge, but that must not discard the proven
                // method target; lowering will cast the receiver back to the
                // method's structural parameter type below.
                let resolved_recursive_receiver =
                    resolved_name.is_some() && type_method.is_some() && receiver_ty == Type::Unknown;
                let (param_types, return_type) = if let Some((_, params, return_type)) =
                    type_method.clone()
                {
                    (params, Arc::new(return_type))
                } else if let Some(signature) =
                    method_signature(receiver, name, self.cx.field_call_signatures)
                {
                    let (params, return_type) = signature;
                    (params, Arc::new(return_type))
                } else {
                    let field_ty = receiver_ty
                        .record_field(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                    let Type::Function {
                        params,
                        return_type,
                        ..
                    } = field_ty
                    else {
                        return Err(Diagnostic::new("attempt to call non-function value"));
                    };
                    (params, return_type)
                };
                if param_types.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "function expects 0 arguments, got {}",
                        args.len() + 1
                    )));
                }
                if !method_receiver_matches(&param_types[0], &receiver_ty)
                    && !resolved_recursive_receiver
                {
                    return Err(Diagnostic::new(format!(
                        "call expected {}, got {}",
                        param_types[0], receiver_ty
                    )));
                }
                let receiver_value =
                    self.lower_expr(receiver, scope, Some(receiver_ty.clone()))?;
                let direct_name = self.direct_record_field_closure_name(receiver_value, name);
                let mut lowered_args = Vec::with_capacity(param_types.len());
                let lowered_receiver = self.coerce_method_receiver(
                    receiver_value,
                    &receiver_ty,
                    &param_types[0],
                )?;
                lowered_args.push(lowered_receiver);
                lowered_args.extend(self.lower_fixed_call_args(
                    args,
                    &param_types[1..],
                    scope,
                )?);
                let value = if let Some((direct_name, _, return_type)) = type_method {
                    if let Some(symbol_id) = self.cx.host_import_names.get(&direct_name) {
                        self.emit(Instruction::HostCall {
                            name: direct_name,
                            symbol_id: *symbol_id,
                            args: lowered_args,
                            return_type,
                        })
                    } else {
                        self.emit(Instruction::Call {
                            name: direct_name,
                            symbol_id: None,
                            args: lowered_args,
                        })
                    }
                } else if let Some(direct_name) = direct_name {
                    self.emit(Instruction::Call {
                        name: direct_name,
                        symbol_id: None,
                        args: lowered_args,
                    })
                } else {
                    let callee_value = self.emit(Instruction::StructGet {
                        base: receiver_value,
                        field: name.clone(),
                        field_ty: Type::Function {
                            params: param_types.clone(),
                            return_type: return_type.clone(),
                            has_self: false,
                        },
                    });
                    self.emit(Instruction::CallValue {
                        callee: callee_value,
                        args: lowered_args,
                        params: param_types.clone(),
                        return_type: Arc::unwrap_or_clone(return_type),
                    })
                };
                self.write_back_method_receiver_mutations(
                    receiver_value,
                    lowered_receiver,
                    &receiver_ty,
                    &param_types[0],
                )?;
                let actual = self.infer_expr_type(expr, &scope.types, None)?;
                self.coerce_value(value, actual, expected)?
            }
            Expr::Unary {
                op,
                expr,
                resolved_name,
                ..
            } => {
                if let Some(name) = resolved_name {
                    let (value, return_type) =
                        self.lower_resolved_operator_call(name, &[expr.as_ref()], scope)?;
                    return self.coerce_value(value, return_type, expected);
                }
                match op {
                    UnaryOp::Neg => {
                        let untyped_actual = first_of_multi(
                            self.infer_expr_type(expr, &scope.types, None)?,
                        );
                        if untyped_actual == Type::Unknown {
                            let boxed = self.lower_expr(expr, scope, Some(Type::Unknown))?;
                            let operand = self.emit(Instruction::DynNumber { value: boxed });
                            let number_ty = Type::Numeric(NumericType::F64);
                            let value = self.emit(Instruction::MathIntrinsic {
                                intrinsic: MathIntrinsic::Neg,
                                args: vec![operand],
                                operand_ty: number_ty.clone(),
                                result_ty: number_ty.clone(),
                            });
                            return self.coerce_value(value, number_ty, expected);
                        }
                        // Only '-' propagates the result's expected type into
                        // its operand (so numeric literals adopt it); 'not'
                        // and '#' operands have unrelated types.
                        let operand_expected = match &expected {
                            Some(Type::Numeric(_)) => expected.clone(),
                            Some(Type::Nullable(inner)) if inner.is_numeric() => {
                                Some((**inner).clone())
                            }
                            _ => None,
                        };
                        let actual =
                            self.infer_expr_type(expr, &scope.types, operand_expected)?;
                        let operand_ty = match actual {
                            Type::Numeric(ty) => ty,
                            Type::StringLiteralUnion(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Bool => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Unit => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::String => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Bytes | Type::Buffer => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Extern | Type::ExternSubtype(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Nil | Type::Nullable(_) | Type::Named { .. } | Type::Opaque { .. } => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Array(_) | Type::Variadic(_) | Type::TypedArray(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Function { .. } | Type::Record(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Multi(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::TypeParam(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                            Type::Thread
                            | Type::Unknown
                            | Type::TaggedVariant(_)
                            | Type::TaggedUnion(_) => {
                                return Err(Diagnostic::new(
                                    "unary '-' requires a numeric operand",
                                ));
                            }
                        };
                        let operand =
                            self.lower_expr(expr, scope, Some(Type::Numeric(operand_ty)))?;
                        let value = if matches!(operand_ty, NumericType::F32 | NumericType::F64) {
                            // Real float negation: `0 - x` would turn -0 into +0.
                            self.emit(Instruction::MathIntrinsic {
                                intrinsic: MathIntrinsic::Neg,
                                args: vec![operand],
                                operand_ty: Type::Numeric(operand_ty),
                                result_ty: Type::Numeric(operand_ty),
                            })
                        } else {
                            let zero = self.emit(Instruction::Number {
                                ty: operand_ty,
                                literal: NumberLiteral {
                                    raw: "0".to_string(),
                                },
                            });
                            self.emit(Instruction::Binary {
                                op: BinaryOp::Sub,
                                left: zero,
                                right: operand,
                                operand_ty: Type::Numeric(operand_ty),
                                result_ty: Type::Numeric(operand_ty),
                            })
                        };
                        self.coerce_value(value, Type::Numeric(operand_ty), expected)?
                    }
                    UnaryOp::Not => {
                        let operand = self.lower_expr(expr, scope, Some(Type::Bool))?;
                        let false_value = self.emit(Instruction::Bool(false));
                        let value = self.emit(Instruction::Binary {
                            op: BinaryOp::Eq,
                            left: operand,
                            right: false_value,
                            operand_ty: Type::Bool,
                            result_ty: Type::Bool,
                        });
                        self.coerce_value(value, Type::Bool, expected)?
                    }
                    UnaryOp::Len => {
                        let actual = self.infer_expr_type(expr, &scope.types, None)?;
                        if actual == Type::String {
                            let value = self.lower_expr(expr, scope, Some(Type::String))?;
                            let len = self.emit_string_host_call(
                                STRING_LEN_HOST,
                                vec![value],
                                Type::Numeric(NumericType::I32),
                            )?;
                            return self.coerce_value(
                                len,
                                Type::Numeric(NumericType::I32),
                                expected,
                            );
                        }
                        if actual == Type::Bytes {
                            let bytes = self.lower_expr(expr, scope, Some(Type::Bytes))?;
                            let len = self.emit(Instruction::BytesLen { bytes });
                            return self.coerce_value(
                                len,
                                Type::Numeric(NumericType::I32),
                                expected,
                            );
                        }
                        if actual == Type::Unknown {
                            let value = self.lower_expr(expr, scope, Some(Type::Unknown))?;
                            let len = self.emit(Instruction::DynLen { value });
                            return self.coerce_value(
                                len,
                                Type::Numeric(NumericType::I32),
                                expected,
                            );
                        }
                        if matches!(actual, Type::TypedArray(_)) {
                            let buffer = self.lower_expr(expr, scope, Some(actual))?;
                            let len = self.emit(Instruction::BufferLen { buffer });
                            return self.coerce_value(
                                len,
                                Type::Numeric(NumericType::I32),
                                expected,
                            );
                        }
                        if !actual.is_array() {
                            return Err(Diagnostic::new(
                                "# requires a string, array, or bytes operand",
                            ));
                        }
                        let array = self.lower_expr(expr, scope, Some(actual))?;
                        let len = self.emit(Instruction::ArrayLen { array });
                        self.coerce_value(len, Type::Numeric(NumericType::I32), expected)?
                    }
                }
            }
            Expr::Cast { expr, ty, .. } => {
                // The unconstrained probe may itself fail (e.g. `{}` needs
                // context); fall through to lowering against the cast target.
                let probed = self.infer_expr_type(expr, &scope.types, None).ok();
                let cast = if let Some(actual) = probed
                    .filter(|actual| require_numeric_cast(actual.clone(), ty.clone()).is_ok())
                {
                    let value = self.lower_expr(expr, scope, None)?;
                    self.explicit_cast(value, actual, ty.clone())?
                } else {
                    let typed_actual = self.infer_expr_type(expr, &scope.types, Some(ty.clone()))?;
                    let value = self.lower_expr(expr, scope, Some(ty.clone()))?;
                    self.coerce_value(value, typed_actual, Some(ty.clone()))?
                };
                self.coerce_value(cast, ty.clone(), expected)?
            }
            Expr::IsVariant { expr, tag, .. } => {
                // Lower the base expression as-is (the underlying IR value is the canonical record).
                let base = self.lower_expr(expr, scope, None)?;
                let tag_id = self.variant_tag_id(tag)?;
                let tag_field_ty = Type::Numeric(NumericType::I32);
                // StructGet "tag" field from the canonical record
                let tag_val = self.emit(Instruction::StructGet {
                    base,
                    field: "tag".to_string(),
                    field_ty: tag_field_ty.clone(),
                });
                let expected_tag = self.emit(Instruction::Number {
                    ty: NumericType::I32,
                    literal: NumberLiteral {
                        raw: tag_id.to_string(),
                    },
                });
                let result = self.emit(Instruction::Binary {
                    op: BinaryOp::Eq,
                    left: tag_val,
                    right: expected_tag,
                    operand_ty: tag_field_ty,
                    result_ty: Type::Bool,
                });
                self.coerce_value(result, Type::Bool, expected)?
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                let result_ty = self.infer_expr_type(expr, &scope.types, expected.clone())?;
                let condition = self.lower_expr(condition, scope, Some(Type::Bool))?;
                let then_block = self.new_block();
                let else_block = self.new_block();
                let merge_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    },
                );

                self.current_block = then_block;
                let then_value = self.lower_expr(then_expr, scope, Some(result_ty.clone()))?;
                let then_exit = self.current_block;
                self.set_terminator(then_exit, Terminator::Jump(merge_block));

                self.current_block = else_block;
                let else_value = self.lower_expr(else_expr, scope, Some(result_ty.clone()))?;
                let else_exit = self.current_block;
                self.set_terminator(else_exit, Terminator::Jump(merge_block));

                self.current_block = merge_block;
                self.emit(Instruction::Phi(vec![
                    (then_exit, then_value),
                    (else_exit, else_value),
                ]))
            }
            Expr::Binary {
                op,
                left,
                right,
                resolved_name,
                ..
            } => match op {
                BinaryOp::And | BinaryOp::Or => {
                    if matches!(op, BinaryOp::Or)
                        && let Some((nullable_ty, result_ty)) =
                            self.nullable_or_types(left, right, &scope.types)?
                    {
                        return self.lower_nullable_or(
                            left, right, nullable_ty, result_ty, scope, expected,
                        );
                    }
                    let result_ty = self.infer_expr_type(expr, &scope.types, expected.clone())?;
                    let left_value = self.lower_expr(left, scope, Some(Type::Bool))?;
                    let rhs_block = self.new_block();
                    let short_block = self.new_block();
                    let merge_block = self.new_block();

                    match op {
                        BinaryOp::And => {
                            self.set_terminator(
                                self.current_block,
                                Terminator::Branch {
                                    condition: left_value,
                                    then_block: rhs_block,
                                    else_block: short_block,
                                },
                            );
                        }
                        BinaryOp::Or => {
                            self.set_terminator(
                                self.current_block,
                                Terminator::Branch {
                                    condition: left_value,
                                    then_block: short_block,
                                    else_block: rhs_block,
                                },
                            );
                        }
                        _ => unreachable!(),
                    }

                    self.current_block = short_block;
                    let short_circuit_value =
                        self.emit(Instruction::Bool(matches!(op, BinaryOp::Or)));
                    let short_exit = self.current_block;
                    self.set_terminator(short_exit, Terminator::Jump(merge_block));

                    self.current_block = rhs_block;
                    let rhs_value = self.lower_expr(right, scope, Some(Type::Bool))?;
                    let rhs_exit = self.current_block;
                    self.set_terminator(rhs_exit, Terminator::Jump(merge_block));

                    self.current_block = merge_block;
                    let mut incoming =
                        vec![(short_exit, short_circuit_value), (rhs_exit, rhs_value)];
                    incoming.sort_by_key(|(pred, _)| *pred);
                    let value = self.emit(Instruction::Phi(incoming));
                    self.coerce_value(value, result_ty, expected)?
                }
                _ => {
                    if let Some(name) = resolved_name {
                        let (value, return_type) = self.lower_resolved_operator_call(
                            name,
                            &[left.as_ref(), right.as_ref()],
                            scope,
                        )?;
                        return self.coerce_value(value, return_type, expected);
                    }
                    if matches!(op, BinaryOp::Eq | BinaryOp::NotEq)
                        && (matches!(left.as_ref(), Expr::Nil(..))
                            || matches!(right.as_ref(), Expr::Nil(..)))
                    {
                        let value_expr = if matches!(left.as_ref(), Expr::Nil(..)) {
                            right
                        } else {
                            left
                        };
                        let nullable_ty =
                            first_of_multi(self.infer_expr_type(value_expr, &scope.types, None)?);
                        if nullable_ty == Type::Nil
                            || matches!(&nullable_ty, Type::Multi(parts) if parts.is_empty())
                        {
                            // An empty multi-value result (e.g. `string.byte`
                            // with a statically empty range) adjusts to nil in
                            // scalar context, so the comparison is statically
                            // decided. The operand is still evaluated for its
                            // side effects.
                            let _ = self.lower_expr(value_expr, scope, None)?;
                            let result =
                                self.emit(Instruction::Bool(matches!(op, BinaryOp::Eq)));
                            return self.coerce_value(result, Type::Bool, expected);
                        }
                        let inner_ty = nullable_ty.nullable_inner().ok_or_else(|| {
                            Diagnostic::new("nil comparison requires a nullable operand")
                        })?;
                        let lowered =
                            self.lower_expr(value_expr, scope, Some(nullable_ty))?;
                        let mut is_null = self.emit(Instruction::IsNull {
                            value: lowered,
                            ty: inner_ty,
                        });
                        if matches!(op, BinaryOp::NotEq) {
                            let false_value = self.emit(Instruction::Bool(false));
                            is_null = self.emit(Instruction::Binary {
                                op: BinaryOp::Eq,
                                left: is_null,
                                right: false_value,
                                operand_ty: Type::Bool,
                                result_ty: Type::Bool,
                            });
                        }
                        return self.coerce_value(is_null, Type::Bool, expected);
                    }
                    let operand_ty =
                        self.infer_binary_operand_type(left, right, op, &scope.types, expected.clone())?;
                    if matches!(operand_ty, Type::Nullable(_))
                        && matches!(op, BinaryOp::Eq | BinaryOp::NotEq)
                    {
                        return self.lower_nullable_equality(
                            *op, left, right, operand_ty, scope, expected,
                        );
                    }
                    let dynamic_numeric = matches!(
                        op,
                        BinaryOp::Add
                            | BinaryOp::Sub
                            | BinaryOp::Mul
                            | BinaryOp::Div
                            | BinaryOp::FloorDiv
                            | BinaryOp::Mod
                            | BinaryOp::Pow
                            | BinaryOp::Less
                            | BinaryOp::LessEq
                            | BinaryOp::Greater
                            | BinaryOp::GreaterEq
                    );
                    let (left, right) = if matches!(op, BinaryOp::Concat)
                        && operand_ty == Type::String
                    {
                        (
                            self.lower_string_concat_operand(left, scope)?,
                            self.lower_string_concat_operand(right, scope)?,
                        )
                    } else if dynamic_numeric {
                        (
                            self.lower_checked_numeric_operand(left, &operand_ty, scope)?,
                            self.lower_checked_numeric_operand(right, &operand_ty, scope)?,
                        )
                    } else {
                        (
                            self.lower_expr(left, scope, Some(operand_ty.clone()))?,
                            self.lower_expr(right, scope, Some(operand_ty.clone()))?,
                        )
                    };
                    // Type the instruction by its operands; the coercion into
                    // a wider expected type (e.g. boxing into `unknown` for a
                    // yield payload) must go through `coerce_value` so the
                    // conversion is actually emitted rather than silently
                    // stamped onto the raw arithmetic result.
                    let raw_result_ty =
                        self.infer_expr_type(expr, &scope.types, Some(operand_ty.clone()))?;
                    let value = self.emit(Instruction::Binary {
                        op: *op,
                        left,
                        right,
                        operand_ty,
                        result_ty: raw_result_ty.clone(),
                    });
                    self.coerce_value(value, raw_result_ty, expected)?
                }
            },
            Expr::Call {
                callee,
                type_args,
                args,
                method_call_origin,
                ..
            } => {
                // Tagged-union constructor: Tag(expr) — emit StructNew for the canonical record.
                if let (Expr::Name(tag, symbol_id, _), [arg]) = (callee.as_ref(), args.as_slice())
                {
                    // symbol_id is None for names that are not resolved to a local or function,
                    // i.e. potential tagged-union constructor names like `Num` or `Flag`.
                    if symbol_id.is_none() {
                        if let Some(variant) = expected
                            .as_ref()
                            .and_then(|e| e.constructed_tagged_variant(tag))
                        {
                            let payload_ty = Arc::unwrap_or_clone(variant.payload);
                            if matches!(&payload_ty, Type::String | Type::Bytes) {
                                return Err(Diagnostic::new(format!(
                                    "tagged union constructor {}({}) is not yet supported: \
                                     string/bytes payloads cannot be boxed into anyref",
                                    tag, payload_ty
                                )));
                            }
                            let payload_val =
                                self.lower_expr(arg, scope, Some(payload_ty.clone()))?;
                            let boxed_val = self.emit(Instruction::Cast {
                                value: payload_val,
                                from: payload_ty,
                                to: Type::Unknown,
                            });
                            let tag_id = self.variant_tag_id(tag)?;
                            let tag_val = self.emit(Instruction::Number {
                                ty: NumericType::I32,
                                literal: NumberLiteral {
                                    raw: tag_id.to_string(),
                                },
                            });
                            let record_ty = Type::canonical_tagged_union_record();
                            let value = self.emit(Instruction::StructNew {
                                struct_ty: record_ty.clone(),
                                fields: vec![tag_val, boxed_val],
                            });
                            // The canonical record IS the runtime representation of any
                            // tagged-union value, so TaggedUnion/TaggedVariant expected types
                            // are satisfied directly.  Only coerce for other targets (e.g.
                            // unknown, or a nullable union: the constructed value is never
                            // nil, but it still has to carry the `Goods?` annotation onward,
                            // and the widening cast is what stamps it on).
                            let coerce_target = match &expected {
                                Some(Type::TaggedUnion(_) | Type::TaggedVariant(_)) => None,
                                other => other.clone(),
                            };
                            return self.coerce_value(value, record_ty, coerce_target);
                        }
                    }
                }
                if let Some(name) = builtin_name(callee.as_ref()) {
                    if let Some(result) = self.lower_json_builtin_call(
                        &name,
                        type_args,
                        args,
                        scope,
                        expected.clone(),
                    ) {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_promise_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_pcall_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_error_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_bit32_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) = self.lower_coroutine_builtin_call(
                        &name,
                        args,
                        scope,
                        expected.clone(),
                    ) {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_select_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_tostring_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_type_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_tonumber_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_table_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) = self.lower_typed_array_builtin_call(
                        &name,
                        args,
                        scope,
                        expected.clone(),
                    ) {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_buffer_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_print_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.lower_string_builtin_call(&name, args, scope, expected.clone())
                    {
                        return result;
                    }
                }
                if let Expr::Name(name, Some(symbol_id), _) = callee.as_ref() {
                    if let Some((param_types, return_type)) =
                        self.cx.host_import_signatures.get(symbol_id).cloned()
                    {
                        let args = self.lower_fixed_call_args(args, &param_types, scope)?;
                        let value = self.emit(Instruction::HostCall {
                            name: name.clone(),
                            symbol_id: *symbol_id,
                            args,
                            return_type,
                        });
                        let actual = self.infer_expr_type(expr, &scope.types, None)?;
                        return self.coerce_value(value, actual, expected);
                    }
                    if let Some((param_types, _)) = self.cx.signatures.get(symbol_id).cloned() {
                        let args = if matches!(
                            param_types.last(),
                            Some(Type::Variadic(element)) if element.as_ref() == &Type::Unknown
                        ) {
                            self.shape_vararg_call_args(&param_types, args, scope)?
                        } else {
                            self.lower_fixed_call_args(args, &param_types, scope)?
                        };
                        let value = self.emit(Instruction::Call {
                            name: name.clone(),
                            symbol_id: Some(*symbol_id),
                            args,
                        });
                        let actual = self.infer_expr_type(expr, &scope.types, None)?;
                        return self.coerce_value(value, actual, expected);
                    }
                }
                if let Some((direct_name, param_types, _)) =
                    direct_field_call_name(callee.as_ref(), self.cx.field_call_signatures)
                {
                    let args = self.lower_fixed_call_args(args, &param_types, scope)?;
                    let value = self.emit(Instruction::Call {
                        name: direct_name,
                        symbol_id: None,
                        args,
                    });
                    let actual = self.infer_expr_type(expr, &scope.types, None)?;
                    return self.coerce_value(value, actual, expected);
                }
                let callee_ty = self.infer_expr_type(callee, &scope.types, None)?;
                let Type::Function {
                    params: param_types,
                    return_type,
                    ..
                } = callee_ty
                else {
                    return Err(Diagnostic::new("attempt to call non-function value"));
                };
                let callee_value = self.lower_expr(
                    callee,
                    scope,
                    Some(Type::Function {
                        params: param_types.clone(),
                        return_type: return_type.clone(),
                        has_self: false,
                    }),
                )?;
                let args = self.lower_fixed_call_args(args, &param_types, scope)?;
                let value = self.emit(Instruction::CallValue {
                    callee: callee_value,
                    args: args.clone(),
                    params: param_types.clone(),
                    return_type: Arc::unwrap_or_clone(return_type),
                });

                // Handle method call writeback if this call originated from a generic method
                if let Some(method_call_origin) = method_call_origin {
                    if !args.is_empty() && !param_types.is_empty() {
                        // Lower the original receiver expression to get the "before coercion" value  
                        let original_receiver_type = self.infer_expr_type(&method_call_origin.original_receiver, &scope.types, None)?;
                        let original_receiver_value = self.lower_expr(
                            &method_call_origin.original_receiver,
                            scope,
                            Some(original_receiver_type.clone()),
                        )?;
                        
                        // The first argument is the coerced receiver that was passed to the method
                        let coerced_receiver_value = args[0];
                        let expected_receiver_type = &param_types[0];
                        
                        // Apply the same writeback logic as used in MethodCall
                        self.write_back_method_receiver_mutations(
                            original_receiver_value,
                            coerced_receiver_value,
                            &original_receiver_type,
                            expected_receiver_type,
                        )?;
                    }
                }

                let actual = self.infer_expr_type(expr, &scope.types, None)?;
                self.coerce_value(value, actual, expected)?
            }
            Expr::Function(function) => {
                let value = self.lower_function_expr(function, scope)?;
                let actual = self.infer_expr_type(expr, &scope.types, None)?;
                self.coerce_value(value, actual, expected)?
            }
            Expr::Require(path, _) => {
                return Err(Diagnostic::new(format!(
                    "unresolved require(\"{path}\") reached IR lowering"
                )));
            }
            Expr::ArrayLiteral { elements, .. } => {
                if let Some(Type::TypedArray(kind)) = expected.as_ref() {
                    return self.lower_typed_array_literal(*kind, elements, scope);
                }
                if elements.is_empty() && expected.as_ref().is_some_and(is_record_like) {
                    let expected_ty = expected.expect("checked above");
                    let record_fields = type_record_fields(&expected_ty)
                        .expect("is_record_like guarantees record fields");
                    // The struct is allocated as the bare record; the final
                    // coercion rewraps opaque aliases and nullable
                    // expectations (`local m: Marker = {}`, `Marker?`).
                    let struct_ty = Type::record(record_fields.clone());
                    let value = self.emit(Instruction::StructNew {
                        struct_ty: struct_ty.clone(),
                        fields: Vec::with_capacity(record_fields.len()),
                    });
                    return self.coerce_value(value, struct_ty, Some(expected_ty));
                }
                let array_ty = self.infer_array_literal_type(elements, &scope.types, expected.clone())?;
                // When the expected type is not an array (e.g. the literal is
                // passed as `unknown`), the coercion above swallows the array
                // shape; recover the literal's own type and let the final
                // coerce_value box it.
                let array_ty = if array_ty.is_array() {
                    array_ty
                } else if let Some(inner) =
                    array_ty.nullable_inner().filter(Type::is_array)
                {
                    // A `{T}?` expectation coerces the inferred array into the
                    // nullable. The literal itself is still the inner array;
                    // the final coerce_value widens it back.
                    inner
                } else {
                    self.infer_array_literal_type(elements, &scope.types, None)?
                };
                let element_ty = array_ty
                    .element_type()
                    .expect("array literal must have element type");
                // Typed without an expectation, so an element that needs one — a
                // tagged-union constructor, say — simply reports "not a pack".
                let trailing_ty = elements
                    .last()
                    .and_then(|element| self.infer_expr_type(element, &scope.types, None).ok());
                let trailing_pack = trailing_ty.as_ref().is_some_and(|ty| match ty {
                    Type::Variadic(_) => true,
                    Type::Multi(parts) => {
                        matches!(parts.last(), Some(Type::Variadic(_)))
                    }
                    _ => false,
                });
                if trailing_pack {
                    // A trailing variadic expression splats after the explicit
                    // prefix; the resulting table is always a fresh copy.
                    let mut prefix = elements[..elements.len() - 1]
                        .iter()
                        .map(|element| self.lower_expr(element, scope, Some(Type::Unknown)))
                        .collect::<Result<Vec<_>, _>>()?;
                    let tail_expr = elements.last().expect("pack requires trailing expression");
                    let pack = match trailing_ty.expect("checked above") {
                        Type::Variadic(element_ty) => self.lower_expr(
                            tail_expr,
                            scope,
                            Some(Type::Variadic(element_ty)),
                        )?,
                        Type::Multi(parts) => {
                            let tuple = self.lower_expr(tail_expr, scope, None)?;
                            let last_index = parts.len() - 1;
                            for (index, part) in parts[..last_index].iter().enumerate() {
                                let value = self.emit(Instruction::MultiGet {
                                    value: tuple,
                                    index,
                                    ty: part.clone(),
                                });
                                prefix.push(self.coerce_value(
                                    value,
                                    part.clone(),
                                    Some(Type::Unknown),
                                )?);
                            }
                            let Type::Variadic(element_ty) = &parts[last_index] else {
                                unreachable!("checked trailing pack above")
                            };
                            self.emit(Instruction::MultiGet {
                                value: tuple,
                                index: last_index,
                                ty: Type::Variadic(element_ty.clone()),
                            })
                        }
                        _ => unreachable!("checked trailing pack above"),
                    };
                    let value = self.emit(Instruction::ArrayNew {
                        element_ty: Type::Unknown,
                        elements: prefix,
                    });
                    self.emit_array_append_all(value, pack);
                    return self.coerce_value(
                        value,
                        Type::Array(Arc::new(Type::Unknown)),
                        expected,
                    );
                }
                let lowered = elements
                    .iter()
                    .map(|element| self.lower_expr(element, scope, Some(element_ty.clone())))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = self.emit(Instruction::ArrayNew {
                    element_ty: to_runtime_type(&element_ty),
                    elements: lowered,
                });
                self.coerce_value(value, array_ty, expected)?
            }
            Expr::Index { base, index, .. } => {
                let base_ty = self.infer_expr_type(base, &scope.types, None)?;
                if base_ty == Type::Bytes {
                    let bytes = self.lower_expr(base, scope, Some(Type::Bytes))?;
                    let index =
                        self.lower_expr(index, scope, Some(Type::Numeric(NumericType::I32)))?;
                    let value = self.emit(Instruction::BytesGet { bytes, index });
                    return self.coerce_value(value, Type::Numeric(NumericType::I32), expected);
                }
                if base_ty == Type::Unknown {
                    let base = self.lower_expr(base, scope, Some(Type::Unknown))?;
                    let index = self.lower_authored_array_index(index, scope)?;
                    let value = self.emit(Instruction::DynIndex { value: base, index });
                    return self.coerce_value(value, Type::Unknown, expected);
                }
                if let Type::TypedArray(kind) = &base_ty {
                    let kind = *kind;
                    let buffer = self.lower_expr(base, scope, Some(base_ty.clone()))?;
                    let index = self.lower_index_to_i32(index, scope)?;
                    let value = self.emit(Instruction::BufferGet {
                        buffer,
                        index,
                        kind,
                    });
                    return self.coerce_value(
                        value,
                        Type::Numeric(kind.element_numeric_type()),
                        expected,
                    );
                }
                let element_ty = base_ty
                    .element_type()
                    .ok_or_else(|| Diagnostic::new("indexing requires an array or bytes operand"))?;
                let array = self.lower_expr(base, scope, Some(base_ty))?;
                let index = self.lower_authored_array_index(index, scope)?;
                let value = self.emit(Instruction::ArrayGet {
                    array,
                    index,
                    // The cell is annotated with what it holds; the read still
                    // carries the source element type onwards, so a tagged union
                    // read out of an array can still be matched on.
                    element_ty: to_runtime_type(&element_ty),
                });
                self.coerce_value(value, element_ty, expected)?
            }
            Expr::TableLiteral { fields, .. } => {
                let inferred_ty = self.infer_expr_type(expr, &scope.types, expected.clone())?;
                let Some(record_fields) = type_record_fields(&inferred_ty) else {
                    return Err(Diagnostic::new(
                        "table literal lowering requires a record type",
                    ));
                };
                let struct_ty = Type::record(record_fields.clone());
                // Record storage is canonicalized by field name, but table
                // constructor expressions evaluate left-to-right as written.
                // Lower into named values first, then assemble StructNew's
                // operands in the record's canonical physical order.
                let mut values_by_name = BTreeMap::new();
                for field in fields {
                    let field_ty = record_fields.get(&field.name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown record field '{}'", field.name))
                    })?;
                    let field_value =
                        self.lower_expr(&field.value, scope, Some(field_ty.clone()))?;
                    values_by_name.insert(field.name.clone(), field_value);
                }
                let mut lowered_fields = Vec::with_capacity(record_fields.len());
                for (name, field_ty) in record_fields {
                    let field_value = if let Some(value) = values_by_name.remove(name) {
                        value
                    } else {
                        self.emit_omitted_nullable_arg(field_ty)?
                    };
                    lowered_fields.push(field_value);
                }
                let value = self.emit(Instruction::StructNew {
                    struct_ty: to_runtime_type(&struct_ty),
                    fields: lowered_fields,
                });
                self.coerce_value(value, struct_ty, expected)?
            }
            Expr::Field {
                base,
                name,
                resolved_name,
                ..
            } => {
                if let Expr::Name(base_name, _, _) = base.as_ref() {
                    let qualified = format!("{base_name}.{name}");
                    if let Some(signature) = self
                        .intrinsic_value_signature_from_expected(&qualified, expected.as_ref())
                    {
                        let (params, return_type) = signature?;
                        return self.lower_intrinsic_value(
                            &qualified,
                            params,
                            return_type,
                            expected,
                        );
                    }
                }
                // Namespace imports are compile-time names, not runtime table
                // values. Taking one as a value creates a typed host adapter
                // before attempting to lower the namespace base.
                if let Expr::Name(base_name, _, _) = base.as_ref() {
                    let qualified = format!("{base_name}.{name}");
                    let expected_function = expected.as_ref().and_then(|ty| match ty {
                        Type::Function {
                            params,
                            return_type,
                            has_self: false,
                        } => Some((params.as_slice(), return_type.as_ref())),
                        _ => None,
                    });
                    let mut candidates = self
                        .cx
                        .host_import_names
                        .iter()
                        .filter_map(|(import_name, symbol_id)| {
                            let is_match = import_name == &qualified
                                || waluau_ast::overload_base_name(import_name)
                                    == Some(qualified.as_str());
                            if !is_match {
                                return None;
                            }
                            let (params, return_type) = self
                                .cx
                                .host_import_signatures
                                .get(symbol_id)?;
                            if let Some((expected_params, expected_return)) = expected_function
                                && (params.as_slice() != expected_params
                                    || return_type != expected_return)
                            {
                                return None;
                            }
                            Some((import_name.clone(), *symbol_id, params.clone(), return_type.clone()))
                        })
                        .collect::<Vec<_>>();
                    candidates.sort_by(|left, right| left.0.cmp(&right.0));
                    if candidates.len() == 1 {
                        let (import_name, symbol_id, params, return_type) = candidates.remove(0);
                        return self.lower_host_import_value(
                            &import_name,
                            symbol_id,
                            params,
                            return_type,
                            expected,
                        );
                    }
                    if candidates.len() > 1 {
                        return Err(Diagnostic::new(format!(
                            "overloaded host function '{qualified}' needs an explicit function type when used as a value"
                        )));
                    }
                    // No host import claimed the name, so an intrinsic with a
                    // single value signature can supply it without an
                    // annotation.
                    if let Some((params, return_type)) =
                        waluau_hir::intrinsic_values::unique_intrinsic_value_signature(&qualified)
                    {
                        return self.lower_intrinsic_value(
                            &qualified,
                            params,
                            return_type,
                            expected,
                        );
                    }
                }
                // A declared namespace constant (`math.pi`) folds to its
                // literal; the namespace is not a value that can be lowered.
                if let Some((constant_ty, literal)) = self.declared_constant(base, name) {
                    let Type::Numeric(numeric) = constant_ty else {
                        return Err(Diagnostic::new(format!(
                            "declared constant '{name}' must have a numeric type"
                        )));
                    };
                    let value = self.emit(Instruction::Number {
                        ty: numeric,
                        literal,
                    });
                    return self.coerce_value(value, Type::Numeric(numeric), expected);
                }
                let base_ty = self.infer_expr_type(base, &scope.types, None)?;
                if let Some((getter_name, params, return_type)) =
                    type_property_getter_signature(
                        &base_ty,
                        resolved_name.as_deref(),
                        name,
                        self.cx.field_call_signatures,
                    )
                {
                    if params.len() != 1 || !method_receiver_matches(&params[0], &base_ty) {
                        return Err(Diagnostic::new(format!(
                            "property getter for '{name}' does not accept receiver {base_ty}"
                        )));
                    }
                    let receiver =
                        self.lower_expr(base, scope, Some(params[0].clone()))?;
                    let symbol_id = self.cx.host_import_names.get(&getter_name).copied().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "declared property getter '{getter_name}' is missing a host import symbol"
                        ))
                    })?;
                    let value = self.emit(Instruction::HostCall {
                        name: getter_name,
                        symbol_id,
                        args: vec![receiver],
                        return_type: return_type.clone(),
                    });
                    return self.coerce_value(value, return_type, expected);
                }
                // Special case: `.value` on a narrowed tagged variant — the IR value is a
                // canonical record, so we must StructGet the `unknown` value field and then
                // Cast (unbox) to the payload type.
                if matches!(&base_ty, Type::TaggedVariant(_)) && name == "value" {
                    let payload_ty = base_ty.record_field(name).expect("TaggedVariant has value field");
                    // Lower base without expected (avoids Record<->TaggedVariant coerce mismatch).
                    let base_val = self.lower_expr(base, scope, None)?;
                    let cast_val = self.unbox_tagged_variant_value(base_val, &payload_ty)?;
                    return self.coerce_value(cast_val, payload_ty, expected);
                }
                // `.n` on an array reads its length, so `table.pack(...).n`
                // behaves like Lua's packed-table count field.
                if matches!(base_ty, Type::Array(_)) && name == "n" {
                    let array = self.lower_expr(base, scope, Some(base_ty))?;
                    let value = self.emit(Instruction::ArrayLen { array });
                    return self.coerce_value(value, Type::Numeric(NumericType::I32), expected);
                }
                let field_ty = base_ty
                    .record_field(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                let storage_field_ty = match base.as_ref() {
                    Expr::Name(_, Some(symbol_id), _) => self
                        .symbol_storage_types
                        .get(symbol_id)
                        .and_then(|ty| ty.record_field(name))
                        .unwrap_or_else(|| field_ty.clone()),
                    _ => field_ty.clone(),
                };
                let base = self.lower_expr(base, scope, Some(base_ty))?;
                let value = self.emit(Instruction::StructGet {
                    base,
                    field: name.clone(),
                    field_ty: storage_field_ty.clone(),
                });
                let value = if storage_field_ty.nullable_inner().as_ref() == Some(&field_ty) {
                    self.emit(Instruction::Cast {
                        value,
                        from: storage_field_ty,
                        to: field_ty.clone(),
                    })
                } else {
                    self.coerce_value(value, storage_field_ty, Some(field_ty.clone()))?
                };
                self.coerce_value(value, field_ty, expected)?
            }
        };
        Ok(value)
    }

    fn lower_expr_list(
        &mut self,
        exprs: &[Expr],
        scope: &Scope,
        expected: Option<&[Type]>,
    ) -> Result<Vec<ValueId>, Diagnostic> {
        let mut out = Vec::new();
        for (expr_index, expr) in exprs.iter().enumerate() {
            let is_last = expr_index + 1 == exprs.len();
            let slot_expected = expected.and_then(|tys| tys.get(out.len()).cloned());
            if let (
                Expr::Call { callee, args, .. },
                Some(expected_ty),
            ) = (expr, slot_expected.as_ref())
            {
                if let (Expr::Name(tag, None, _), [_]) = (callee.as_ref(), args.as_slice()) {
                    if expected_ty.constructed_tagged_variant(tag).is_some() {
                        out.push(self.lower_expr(
                            expr,
                            scope,
                            Some(expected_ty.clone()),
                        )?);
                        continue;
                    }
                }
            }
            let remaining_expected = expected
                .and_then(|tys| tys.get(out.len()..))
                .filter(|tys| !tys.is_empty());
            let ty = if let Expr::Call { callee, .. } = expr {
                let expands_multi = builtin_name(callee.as_ref()).is_some_and(|name| {
                    name == COROUTINE_RESUME
                        || name == PCALL
                        || name == STRING_FIND
                        || name == STRING_MATCH
                        || name == STRING_GSUB
                        || name == STRING_BYTE
                        || name == JSON_UNPACK
                        || name == TABLE_UNPACK
                });
                let is_potential_constructor =
                    matches!(callee.as_ref(), Expr::Name(_, None, _));
                if expands_multi {
                    self.infer_expr_type(
                        expr,
                        &scope.types,
                        remaining_expected.map(|tys| Type::Multi(tys.to_vec())),
                    )?
                } else if is_potential_constructor {
                    self.infer_expr_type(expr, &scope.types, slot_expected.clone())?
                } else {
                    self.infer_expr_type(expr, &scope.types, None)?
                }
            } else if matches!(expr, Expr::MethodCall { .. }) {
                self.infer_expr_type(expr, &scope.types, None)?
            } else {
                self.infer_expr_type(expr, &scope.types, slot_expected.clone())?
            };
            // A no-result call still executes for effects. In final position
            // it contributes no values; in an earlier position Lua adjusts it
            // to one nil value.
            if ty == Type::Unit && matches!(expr, Expr::Call { .. } | Expr::MethodCall { .. }) {
                let _ = self.lower_expr(expr, scope, Some(Type::Unit))?;
                if !is_last {
                    let nil_ty = slot_expected.unwrap_or(Type::Nil);
                    out.push(self.lower_expr(&Expr::Nil(None), scope, Some(nil_ty))?);
                }
                continue;
            }
            match ty {
                Type::Variadic(element_ty) => {
                    let pack = self.lower_expr(
                        expr,
                        scope,
                        Some(Type::Variadic(element_ty.clone())),
                    )?;
                    if matches!(
                        remaining_expected.and_then(|tys| tys.first()),
                        Some(Type::Variadic(_))
                    ) || remaining_expected.is_none()
                    {
                        out.push(pack);
                    } else if let Some(expected_types) = remaining_expected {
                        for (index, expected_ty) in expected_types.iter().enumerate() {
                            let index = self.emit_i32_const(index as i32);
                            let value = self.emit(Instruction::ArrayGet {
                                array: pack,
                                index,
                                element_ty: (*element_ty).clone(),
                            });
                            out.push(self.coerce_value(
                                value,
                                (*element_ty).clone(),
                                Some(expected_ty.clone()),
                            )?);
                        }
                    }
                }
                Type::Multi(multi_types) => {
                    // Inference has already chosen this tuple's exact static
                    // shape, potentially from the remaining expected binding
                    // slots. Preserve that shape during lowering for every
                    // multi-value expression; this does not turn a runtime
                    // variadic result into a statically sized tuple.
                    let tuple = self.lower_expr(
                        expr,
                        scope,
                        Some(Type::Multi(multi_types.clone())),
                    )?;
                    // Lua adjusts a multi-value expression to its first value
                    // in every position but the last of an expression list;
                    // only the final expression expands. Packs (a leading
                    // Variadic) keep the existing forwarding behavior.
                    if !is_last
                        && matches!(
                            multi_types.first(),
                            Some(first) if !matches!(first, Type::Variadic(_))
                        )
                    {
                        let mut multi_types = multi_types;
                        let first = multi_types.remove(0);
                        let coerced = if let Some(exp) = expected.and_then(|tys| tys.get(out.len()))
                        {
                            coerce_type(first, Some(exp.clone()))?
                        } else {
                            first
                        };
                        out.push(self.emit(Instruction::MultiGet {
                            value: tuple,
                            index: 0,
                            ty: coerced,
                        }));
                        continue;
                    }
                    for (index, part) in multi_types.into_iter().enumerate() {
                        if let Type::Variadic(element_ty) = part {
                            let pack = self.emit(Instruction::MultiGet {
                                value: tuple,
                                index,
                                ty: Type::Variadic(element_ty.clone()),
                            });
                            let remaining = expected
                                .and_then(|tys| tys.get(out.len()..))
                                .filter(|tys| !tys.is_empty());
                            if matches!(
                                remaining.and_then(|tys| tys.first()),
                                Some(Type::Variadic(_))
                            ) || remaining.is_none()
                            {
                                out.push(pack);
                            } else if let Some(expected_types) = remaining {
                                for (offset, expected_ty) in expected_types.iter().enumerate() {
                                    let index = self.emit_i32_const(offset as i32);
                                    let value = self.emit(Instruction::ArrayGet {
                                        array: pack,
                                        index,
                                        element_ty: (*element_ty).clone(),
                                    });
                                    out.push(self.coerce_value(
                                        value,
                                        (*element_ty).clone(),
                                        Some(expected_ty.clone()),
                                    )?);
                                }
                            }
                            continue;
                        }
                        let coerced = if let Some(exp) =
                            expected.and_then(|tys| tys.get(out.len()))
                        {
                            coerce_type(part, Some(exp.clone()))?
                            } else {
                                part
                            };
                        let value = self.emit(Instruction::MultiGet {
                            value: tuple,
                            index,
                            ty: coerced,
                        });
                        out.push(value);
                    }
                }
                scalar => {
                    let coerced = if let Some(exp) = expected.and_then(|tys| tys.get(out.len()))
                    {
                        coerce_type(scalar, Some(exp.clone()))?
                    } else {
                        scalar
                    };
                    let value = self.lower_expr(expr, scope, Some(coerced))?;
                    out.push(value);
                }
            }
        }
        Ok(out)
    }

    fn lower_function_expr(
        &mut self,
        function: &waluau_ast::FunctionExpr,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        let return_ty = Self::function_expr_return_type(function)?;
        let captures = collect_captures(function, &scope.env, &scope.types, self.cx.signatures);
        let capture_values = captures
            .iter()
            .map(|(symbol_id, _)| {
                scope.env.get(symbol_id).copied().ok_or_else(|| {
                    Diagnostic::new(format!("unknown local symbol ID {:?} during IR lowering", symbol_id))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let lifted_name = format!("{}$lambda{}", self.function.name, self.lambda_counter);
        self.lambda_counter += 1;

        let capture_count = captures.len();
        let source_file = self
            .cx
            .source_file_ids
            .get(&function.file_path)
            .copied()
            .expect("lifted function source must be interned");
        let mut lifted = Function {
            name: lifted_name.clone(),
            params: Vec::new(),
            return_type: return_ty.clone(),
            entry: BlockId(0),
            blocks: BTreeMap::new(),
            next_value: 0,
            capture_count,
            value_symbols: BTreeMap::new(),
            symbol_id: function.symbol_id,
            source_map: FunctionSourceMap {
                definition: function.span.map_or(SourceOrigin::Synthetic, |span| {
                    SourceOrigin::Authored(SourceLocation {
                        file: source_file,
                        span,
                    })
                }),
                instruction_locations: BTreeMap::new(),
                terminator_locations: BTreeMap::new(),
            },
        };
        lifted.blocks.insert(
            lifted.entry,
            BasicBlock {
                id: lifted.entry,
                instructions: Vec::new(),
                terminator: Terminator::Unreachable { span: None },
            },
        );
        for (symbol_id, ty) in &captures {
            // Captured variables are passed as 1-element array "cells" to nested
            // (lifted) functions so they can observe/mutate shared storage.
            lifted.params.push((
                format!("capture_{}", symbol_id.0),
                Type::Array(Arc::new(to_runtime_type(ty))),
            ));
        }
        for param in &function.params {
            lifted.params.push((param.name.clone(), param.ty.clone()));
        }

        let mut nested_env = HashMap::new();
        let mut nested_types = self
            .cx
            .globals
            .iter()
            .map(|(symbol_id, (_, ty))| (*symbol_id, ty.clone()))
            .collect::<HashMap<_, _>>();
        let lifted_entry = lifted.entry;
        for (index, (symbol_id, ty)) in captures.iter().cloned().enumerate() {
            let value = lifted.next_value();
            block_mut(&mut lifted, lifted_entry)
                .instructions
                .push((value, Instruction::Param(index)));
            nested_env.insert(symbol_id, value);
            lifted.value_symbols.insert(value, symbol_id);
            // `ty` is the captured variable's source-level type (cell-backed
            // storage keeps the inner type in the type map), so record it
            // as-is. Reads still go through the cell via ArrayGet; unwrapping
            // here would corrupt captures that are themselves arrays.
            nested_types.insert(symbol_id, ty);
        }
        // Names that this closure's own nested functions capture; the ones that
        // are our own params must be wrapped into cells below, just like the
        // params of named functions.
        let nested_inner_captures = collect_nested_function_capture_names(&waluau_ast::Function {
            name: waluau_ast::FunctionName::Simple(function.name.clone().unwrap_or_default()),
            declaration_class: waluau_ast::FunctionDeclarationClass::Module,
            symbol_id: function.symbol_id,
            type_params: function.type_params.clone(),
            params: function.params.clone(),
            vararg: function.vararg.clone(),
            return_type: Some(return_ty.clone()),
            body: function.body.clone(),
            file_path: function.file_path.clone(),
            span: function.span,
        });

        let captures_count = captures.len();
        for (index, param) in function.params.iter().enumerate() {
            let symbol_id = param.symbol_id.expect("param has resolved symbol_id");
            let value = lifted.next_value();
            block_mut(&mut lifted, lifted_entry)
                .instructions
                .push((value, Instruction::Param(captures_count + index)));
            if nested_inner_captures.contains(&symbol_id) {
                // This param is captured by an inner closure: box it into a
                // 1-element cell so the inner closure shares its storage.
                let cell = lifted.next_value();
                block_mut(&mut lifted, lifted_entry).instructions.push((
                    cell,
                    Instruction::ArrayNew {
                        element_ty: to_runtime_type(&param.ty),
                        elements: vec![value],
                    },
                ));
                nested_env.insert(symbol_id, cell);
                lifted.value_symbols.insert(cell, symbol_id);
            } else {
                nested_env.insert(symbol_id, value);
            }
            lifted.value_symbols.insert(value, symbol_id);
            nested_types.insert(symbol_id, param.ty.clone());
        }

        // nested builder should treat the capture parameters as cell-backed names
        // so the nested function will access them via ArrayGet/ArraySet.
        let mut capture_param_symbols: HashSet<SymbolId> =
            captures.iter().map(|(id, _)| *id).collect();
        capture_param_symbols.extend(nested_inner_captures);

        let symbol_storage_types = nested_types.clone();
        let mut nested = Builder {
            function: lifted,
            current_block: BlockId(0),
            next_block: 1,
            cx: self.cx,
            lifted_functions: Vec::new(),
            lambda_counter: 0,
            loop_stack: Vec::new(),
            cell_names: capture_param_symbols,
            file_path: function.file_path.clone(),
            source_file: function.span.map(|_| source_file),
            current_span: None,
            vararg_value: None,
            vararg_element: None,
            discriminants: HashMap::new(),
            symbol_storage_types,
        };
        let has_private_self_binding = !matches!(
            function.declaration_class,
            Some(
                waluau_ast::FunctionDeclarationClass::Local
                    | waluau_ast::FunctionDeclarationClass::Const
            )
        );
        if has_private_self_binding && function.name.is_some() {
            let symbol_id = function.symbol_id.expect("resolved symbol_id");
            let capture_param_values = captures
                .iter()
                .map(|(capture_symbol_id, _)| {
                    nested_env.get(capture_symbol_id).copied().ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing capture symbol ID {:?} in nested function lowering",
                            capture_symbol_id
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let self_callee = nested.emit(Instruction::Closure {
                name: lifted_name.clone(),
                captures: capture_param_values,
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
                return_type: return_ty.clone(),
            });
            nested_env.insert(symbol_id, self_callee);
            nested.function.value_symbols.insert(self_callee, symbol_id);
            nested_types.insert(
                symbol_id,
                Type::Function {
                    params: function
                        .params
                        .iter()
                        .map(|param| param.ty.clone())
                        .collect(),
                    return_type: Arc::new(return_ty.clone()),
                    has_self: false,
                },
            );
        }
        let mut nested_scope = Scope {
            env: nested_env,
            types: nested_types,
        };
        for stmt in &function.body {
            if nested.current_block == DEAD_BLOCK {
                break;
            }
            nested.lower_stmt(stmt, &mut nested_scope)?;
        }
        if nested.current_block != DEAD_BLOCK && nested.function.return_type == Type::Unit {
            let value = nested.emit(Instruction::Unit);
            nested.set_terminator(nested.current_block, Terminator::Return(value));
            nested.current_block = DEAD_BLOCK;
        }
        if nested.current_block != DEAD_BLOCK
            && matches!(nested.function.return_type, Type::Nullable(_))
        {
            let return_type = nested.function.return_type.clone();
            let value = nested.emit(Instruction::Null { ty: return_type });
            nested.set_terminator(nested.current_block, Terminator::Return(value));
            nested.current_block = DEAD_BLOCK;
        }
        self.lifted_functions.push(nested.function);
        self.lifted_functions.extend(nested.lifted_functions);

        Ok(self.emit(Instruction::Closure {
            name: lifted_name,
            captures: capture_values,
            params: function
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect(),
            return_type: return_ty,
        }))
    }

    /// The declared namespace constant a field access reads, when `base` is a
    /// bare namespace name and `base.name` is a declared constant (mirrors
    /// the qualified-name lookup HIR performs before inferring the base).
    fn declared_constant(&self, base: &Expr, name: &str) -> Option<(Type, NumberLiteral)> {
        let Expr::Name(base_name, _, _) = base else {
            return None;
        };
        self.cx.declared_constants
            .get(&format!("{base_name}.{name}"))
            .cloned()
    }

    fn infer_expr_type(
        &self,
        expr: &Expr,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Number(..) => match expected {
                Some(Type::Numeric(ty)) => Ok(Type::Numeric(ty)),
                Some(Type::Bool) => {
                    Err(Diagnostic::new("numeric literal is not assignable to bool"))
                }
                Some(Type::Unit) => {
                    Err(Diagnostic::new("numeric literal is not assignable to unit"))
                }
                Some(Type::String) => Err(Diagnostic::new(
                    "numeric literal is not assignable to string",
                )),
                Some(Type::Bytes) => Err(Diagnostic::new(
                    "numeric literal is not assignable to bytes",
                )),
                Some(Type::Buffer) => Err(Diagnostic::new(
                    "numeric literal is not assignable to buffer",
                )),
                Some(Type::Extern) | Some(Type::ExternSubtype(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to extern",
                )),
                Some(Type::Nil) => Err(Diagnostic::new(
                    "numeric literal is not assignable to nil",
                )),
                Some(Type::Nullable(inner)) => match Arc::unwrap_or_clone(inner) {
                    Type::Numeric(numeric) => {
                        Ok(Type::Nullable(Arc::new(Type::Numeric(numeric))))
                    }
                    other => Err(Diagnostic::new(format!(
                        "numeric literal is not assignable to {other}?",
                    ))),
                },
                Some(Type::Named { name, .. }) => Err(Diagnostic::new(format!(
                    "numeric literal is not assignable to {name}",
                ))),
                Some(Type::Opaque { name, .. }) => Err(Diagnostic::new(format!(
                    "numeric literal is not assignable to {name}",
                ))),
                Some(Type::Array(_) | Type::Variadic(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to array",
                )),
                Some(Type::TypedArray(kind)) => Err(Diagnostic::new(format!(
                    "numeric literal is not assignable to {}",
                    kind.type_name(),
                ))),
                Some(Type::Function { .. }) => Err(Diagnostic::new(
                    "numeric literal is not assignable to function",
                )),
                Some(Type::Multi(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to multiple values",
                )),
                Some(Type::Record(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to namespace",
                )),
                Some(Type::TypeParam(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to generic type parameter",
                )),
                Some(Type::Thread) => Err(Diagnostic::new(
                    "numeric literal is not assignable to thread",
                )),
                Some(Type::TaggedVariant(_)) | Some(Type::TaggedUnion(_)) => Err(
                    Diagnostic::new("numeric literal is not assignable to tagged union type"),
                ),
                Some(Type::StringLiteralUnion(_)) => Err(Diagnostic::new(
                    "numeric literal is not assignable to string",
                )),
                // A literal coerced to `unknown` is boxed; report `unknown` as its type.
                Some(Type::Unknown) => Ok(Type::Unknown),
                None => Ok(Type::number()),
            },
            Expr::Nil(..) => coerce_type(Type::Nil, expected),
            Expr::IsVariant { .. } => coerce_type(Type::Bool, expected),
            Expr::Bool(..) => coerce_type(Type::Bool, expected),
            Expr::String(..) => coerce_type(Type::String, expected),
            Expr::Bytes(..) => coerce_type(Type::Bytes, expected),
            Expr::Vararg(..) => Ok(Type::Variadic(Arc::new(
                self.vararg_element.clone().unwrap_or(Type::Unknown),
            ))),
            Expr::Require(path, _) => Err(Diagnostic::new(format!(
                "unresolved require(\"{path}\") reached IR lowering"
            ))),
            Expr::Name(name, symbol_id, _) => {
                // A name with no symbol is normally a tagged-union constructor being
                // probed without the expected type that would resolve it. Callers that
                // probe types recover from this, so it is a diagnostic, not a crash.
                let Some(symbol_id) = *symbol_id else {
                    return Err(Diagnostic::new(format!("unknown name '{name}'")));
                };
                if let Some(ty) = types.get(&symbol_id) {
                    // Coerce toward the expected type when possible (dynamic
                    // `unknown` values, nullable widening, ...); callers that
                    // only probe the raw type still get it on mismatch.
                    Ok(coerce_type(ty.clone(), expected).unwrap_or_else(|_| ty.clone()))
                } else if let Some((params, ret)) = self.cx.signatures.get(&symbol_id) {
                    Ok(Type::Function {
                        params: params.clone(),
                        return_type: Arc::new(ret.clone()),
                        has_self: false,
                    })
                } else if let Some(signature) =
                    self.intrinsic_value_signature_from_expected(name, expected.as_ref())
                {
                    let (params, return_type) = signature?;
                    Ok(function_type(params, return_type))
                } else if let Some((params, return_type)) =
                    waluau_hir::intrinsic_values::unique_intrinsic_value_signature(name)
                {
                    Ok(function_type(params, return_type))
                } else {
                    Err(Diagnostic::new(format!(
                        "unknown local '{name}' during IR lowering"
                    )))
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                resolved_name,
                args,
                ..
            } => {
                let receiver_ty = self.infer_expr_type(receiver, types, None)?;
                if receiver_ty == Type::String {
                    let mut call_args = Vec::with_capacity(args.len() + 1);
                    call_args.push((**receiver).clone());
                    call_args.extend_from_slice(args);
                    let builtin = match name.as_str() {
                        "find" => Some(STRING_FIND),
                        "match" => Some(STRING_MATCH),
                        "gmatch" => Some(STRING_GMATCH),
                        "gsub" => Some(STRING_GSUB),
                        "len" => Some(STRING_LEN),
                        "sub" => Some(STRING_SUB),
                        "rep" => Some(STRING_REP),
                        "byte" => Some(STRING_BYTE),
                        "upper" => Some(STRING_UPPER),
                        "lower" => Some(STRING_LOWER),
                        "format" => Some(STRING_FORMAT),
                        "reverse" => Some(STRING_REVERSE),
                        "split" => Some(STRING_SPLIT),
                        _ => None,
                    };
                    if let Some(builtin) = builtin {
                    if let Some(result) =
                        self.infer_string_builtin_call_type(builtin, &call_args, types, expected.clone())
                    {
                        return result;
                    }
                    }
                }
                if let Some(result) =
                    self.infer_promise_await_method_call_type(name, args, expected.clone())
                {
                    return result;
                }
                let type_method = type_method_signature(
                    &receiver_ty,
                    resolved_name.as_deref(),
                    name,
                    self.cx.field_call_signatures,
                );
                // See the lowering path above: HIR's resolved target is the
                // source-level proof for an `unknown` recursive runtime edge.
                let resolved_recursive_receiver =
                    resolved_name.is_some() && type_method.is_some() && receiver_ty == Type::Unknown;
                let (params, return_type) = if let Some((_, params, return_type)) = type_method {
                    (params, Arc::new(return_type))
                } else if let Some(signature) =
                    method_signature(receiver, name, self.cx.field_call_signatures)
                {
                    let (params, return_type) = signature;
                    (params, Arc::new(return_type))
                } else {
                    let field_ty = receiver_ty
                        .record_field(name)
                        .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                    let Type::Function {
                        params,
                        return_type,
                        ..
                    } = field_ty
                    else {
                        return Err(Diagnostic::new("attempt to call non-function value"));
                    };
                    (params, return_type)
                };
                if params.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "function expects 0 arguments, got {}",
                        args.len() + 1
                    )));
                }
                if !method_receiver_matches(&params[0], &receiver_ty)
                    && !resolved_recursive_receiver
                {
                    return Err(Diagnostic::new(format!(
                        "call expected {}, got {}",
                        params[0], receiver_ty
                    )));
                }
                if !call_arity_matches(&params, args.len() + 1) {
                    return Err(Diagnostic::new(format!(
                        "function expects {} arguments, got {}",
                        params.len(),
                        args.len() + 1
                    )));
                }
                // Infer each argument against its parameter type so literals
                // resolve to the parameter's numeric type (a bare integer
                // literal would otherwise default to f64 and mismatch).
                for (expected_param, arg) in params.iter().skip(1).zip(args.iter()) {
                    let actual =
                        self.infer_expr_type(arg, types, Some(expected_param.clone()))?;
                    if coerce_type(actual.clone(), Some(expected_param.clone())).is_err() {
                        return Err(Diagnostic::new(format!(
                            "call expected {}, got {}",
                            expected_param, actual
                        )));
                    }
                }
                coerce_type(Arc::unwrap_or_clone(return_type), expected)
            }
            Expr::Unary {
                op,
                expr,
                resolved_name,
                ..
            } => match op {
                UnaryOp::Neg => {
                    if let Some(name) = resolved_name {
                        let (_, return_type) = self.cx.field_call_signatures.get(name).cloned().ok_or_else(
                            || Diagnostic::new(format!("operator overload '{name}' is not declared")),
                        )?;
                        return coerce_type(return_type, expected);
                    }
                    let untyped_actual = first_of_multi(
                        self.infer_expr_type(expr, types, None)?,
                    );
                    if untyped_actual == Type::Unknown {
                        return coerce_type(Type::number(), expected);
                    }
                    let operand_expected = match &expected {
                        Some(Type::Numeric(_)) => expected.clone(),
                        Some(Type::Nullable(inner)) if inner.is_numeric() => {
                            Some((**inner).clone())
                        }
                        _ => None,
                    };
                    let actual = self.infer_expr_type(expr, types, operand_expected)?;
                    match actual {
                        Type::Numeric(_) => coerce_type(actual, expected),
                        Type::Bool => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                        Type::Unit => Err(Diagnostic::new("unary '-' requires a numeric operand")),
                        Type::String => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Bytes | Type::Buffer => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Extern | Type::ExternSubtype(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Nil | Type::Nullable(_) | Type::Named { .. } | Type::Opaque { .. } => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Array(_) | Type::Variadic(_) | Type::TypedArray(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Function { .. } | Type::Record(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Multi(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::TypeParam(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                        Type::Thread
                        | Type::Unknown
                        | Type::TaggedVariant(_)
                        | Type::TaggedUnion(_)
                        | Type::StringLiteralUnion(_) => {
                            Err(Diagnostic::new("unary '-' requires a numeric operand"))
                        }
                    }
                }
                UnaryOp::Not => {
                    let actual = self.infer_expr_type(expr, types, Some(Type::Bool))?;
                    if actual == Type::Bool {
                        Ok(Type::Bool)
                    } else {
                        Err(Diagnostic::new("unary 'not' requires a bool operand"))
                    }
                }
                UnaryOp::Len => {
                    let actual = self.infer_expr_type(expr, types, None)?;
                    if actual == Type::String
                        || actual == Type::Bytes
                        || actual == Type::Unknown
                        || actual.is_array()
                        || matches!(actual, Type::TypedArray(_))
                    {
                        coerce_type(Type::Numeric(NumericType::I32), expected)
                    } else {
                        Err(Diagnostic::new("# requires a string, array, or bytes operand"))
                    }
                }
            },
            Expr::Cast { expr, ty, .. } => {
                // The unconstrained probe may itself fail (e.g. `{}` needs
                // context); fall through to inference against the cast target.
                let numeric_cast = self
                    .infer_expr_type(expr, types, None)
                    .is_ok_and(|actual| require_numeric_cast(actual, ty.clone()).is_ok());
                if !numeric_cast {
                    self.infer_expr_type(expr, types, Some(ty.clone()))?;
                }
                Ok(ty.clone())
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                let condition_ty = self.infer_expr_type(condition, types, Some(Type::Bool))?;
                if condition_ty != Type::Bool {
                    return Err(Diagnostic::new("if expression condition must be bool"));
                }
                let then_ty = self.infer_expr_type(then_expr, types, expected.clone())?;
                let else_ty = self.infer_expr_type(else_expr, types, expected.clone())?;
                if then_ty == else_ty {
                    Ok(then_ty)
                } else {
                    Err(Diagnostic::new(
                        "if expression branches must resolve to the same type",
                    ))
                }
            }
            Expr::Call {
                callee,
                type_args,
                args,
                ..
            } => {
                // Tagged-union constructor type inference mirrors lower_expr detection.
                if let (Expr::Name(tag, symbol_id, _), [_arg]) = (callee.as_ref(), args.as_slice())
                {
                    if symbol_id.is_none() {
                        if let Some(variant) = expected
                            .as_ref()
                            .and_then(|e| e.constructed_tagged_variant(tag))
                        {
                            let result_ty = Type::TaggedVariant(TaggedVariant {
                                tag: variant.tag.clone(),
                                payload: variant.payload.clone(),
                            });
                            return coerce_type(result_ty, expected);
                        }
                    }
                }
                if let Some(name) = builtin_name(callee.as_ref()) {
                    if let Some(result) = self.infer_json_builtin_call_type(
                        &name,
                        type_args,
                        args,
                        types,
                        expected.clone(),
                    ) {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_promise_builtin_call_type(&name, args, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_pcall_builtin_call_type(&name, args, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_error_builtin_call_type(&name, args, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) = self.infer_bit32_builtin_call_type(&name, args, types) {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_coroutine_builtin_call_type(&name, expr, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) = self.infer_select_builtin_call_type(&name, args, types)
                    {
                        return result;
                    }
                    if let Some(result) = self.infer_tostring_builtin_call_type(&name, expr, types)
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_type_builtin_call_type(&name, args, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_tonumber_builtin_call_type(&name, args, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_table_builtin_call_type(&name, expr, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_typed_array_builtin_call_type(&name, args, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_buffer_builtin_call_type(&name, args, types, expected.clone())
                    {
                        return result;
                    }
                    if let Some(result) = self.infer_print_builtin_call_type(&name, expr, types) {
                        return result;
                    }
                    if let Some(result) =
                        self.infer_string_builtin_call_type(&name, args, types, expected.clone())
                    {
                        return result;
                    }
                }
                let callee_ty = self.infer_expr_type(callee, types, None)?;
                match callee_ty {
                    // Coerce the return type against the expected type so a
                    // call result adapts like any other expression (e.g. an
                    // extern result in an extern? argument position), matching
                    // the HIR inference path.
                    Type::Function { return_type, .. } => coerce_type(Arc::unwrap_or_clone(return_type), expected),
                    other => Err(Diagnostic::new(format!(
                        "attempt to call non-function value of type {other}",
                    ))),
                }
            }
            Expr::Function(function) => Ok(Type::Function {
                has_self: false,
                return_type: Arc::new(Self::function_expr_return_type(function)?),
                params: function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
            }),
            Expr::ArrayLiteral { elements, .. } => {
                self.infer_array_literal_type(elements, types, expected)
            }
            Expr::TableLiteral { fields, .. } => {
                let mut record_fields = BTreeMap::new();
                let expected_fields = expected.as_ref().and_then(type_record_fields);
                for field in fields {
                    let expected_field_ty = expected_fields
                        .and_then(|fields| fields.get(&field.name))
                        .cloned();
                    let field_ty = self.infer_expr_type(&field.value, types, expected_field_ty)?;
                    record_fields.insert(field.name.clone(), field_ty);
                }
                coerce_type(Type::record(record_fields), expected)
            }
            Expr::Field {
                base,
                name,
                resolved_name,
                ..
            } => {
                if let Expr::Name(base_name, _, _) = base.as_ref() {
                    let qualified = format!("{base_name}.{name}");
                    if let Some(signature) = self
                        .intrinsic_value_signature_from_expected(&qualified, expected.as_ref())
                    {
                        let (params, return_type) = signature?;
                        return Ok(Type::Function {
                            params,
                            return_type: Arc::new(return_type),
                            has_self: false,
                        });
                    }
                    let mut candidates = self
                        .cx
                        .host_import_names
                        .iter()
                        .filter(|&(import_name, _symbol_id)| import_name == &qualified
                                || waluau_ast::overload_base_name(import_name)
                                    == Some(qualified.as_str())).map(|(_import_name, symbol_id)| {
                                let (params, return_type) = self
                                    .cx
                                    .host_import_signatures
                                    .get(symbol_id)
                                    .expect("host import name has a signature");
                                Type::Function {
                                    params: params.clone(),
                                    return_type: Arc::new(return_type.clone()),
                                    has_self: false,
                                }
                            })
                        .filter(|candidate| {
                            expected.as_ref().is_none_or(|expected| candidate == expected)
                        })
                        .collect::<Vec<_>>();
                    candidates.dedup();
                    if candidates.len() == 1 {
                        return coerce_type(candidates.remove(0), expected);
                    }
                    if candidates.len() > 1 {
                        return Err(Diagnostic::new(format!(
                            "overloaded host function '{qualified}' needs an explicit function type when used as a value"
                        )));
                    }
                    if let Some((params, return_type)) =
                        waluau_hir::intrinsic_values::unique_intrinsic_value_signature(&qualified)
                    {
                        return coerce_type(function_type(params, return_type), expected);
                    }
                }
                if let Some((constant_ty, _)) = self.declared_constant(base, name) {
                    return coerce_type(constant_ty, expected);
                }
                let base_ty = self.infer_expr_type(base, types, None)?;
                if let Some((_, params, return_type)) =
                    type_property_getter_signature(
                        &base_ty,
                        resolved_name.as_deref(),
                        name,
                        self.cx.field_call_signatures,
                    )
                {
                    if params.len() == 1 && method_receiver_matches(&params[0], &base_ty) {
                        return coerce_type(return_type, expected);
                    }
                }
                if matches!(base_ty, Type::Array(_)) && name == "n" {
                    return coerce_type(Type::Numeric(NumericType::I32), expected);
                }
                let field_ty = base_ty
                    .record_field(name)
                    .ok_or_else(|| Diagnostic::new(format!("unknown record field '{name}'")))?;
                coerce_type(field_ty, expected)
            }
            Expr::Index { base, index, .. } => {
                let base_ty = self.infer_expr_type(base, types, None)?;
                let element_ty = if base_ty == Type::Bytes {
                    Type::Numeric(NumericType::I32)
                } else if base_ty == Type::Unknown {
                    Type::Unknown
                } else {
                    base_ty
                        .element_type()
                        .ok_or_else(|| Diagnostic::new("indexing requires an array or bytes operand"))?
                };
                let index_ty = self
                    .infer_expr_type(index, types, None)
                    .map(first_of_multi)
                    .or_else(|_| {
                        self.infer_expr_type(index, types, Some(Type::Numeric(NumericType::I32)))
                    })?;
                if !index_ty.is_numeric() && index_ty != Type::Unknown {
                    return Err(Diagnostic::new("array index must be numeric"));
                }
                coerce_type(element_ty, expected)
            }
            Expr::Binary {
                op,
                left,
                right,
                resolved_name,
                ..
            } => match op {
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::FloorDiv
                | BinaryOp::Mod
                | BinaryOp::Pow
                | BinaryOp::Concat => {
                    if let Some(name) = resolved_name {
                        let (_, return_type) = self.cx.field_call_signatures.get(name).cloned().ok_or_else(
                            || Diagnostic::new(format!("operator overload '{name}' is not declared")),
                        )?;
                        return coerce_type(return_type, expected);
                    }
                    let raw = self.infer_binary_operand_type(left, right, op, types, expected.clone())?;
                    coerce_type(raw, expected)
                }
                BinaryOp::Or => {
                    if let Some((_, result_ty)) = self.nullable_or_types(left, right, types)? {
                        return coerce_type(result_ty, expected);
                    }
                    Ok(Type::Bool)
                }
                BinaryOp::Less
                | BinaryOp::LessEq
                | BinaryOp::Greater
                | BinaryOp::GreaterEq
                | BinaryOp::Eq
                | BinaryOp::NotEq
                | BinaryOp::And => Ok(Type::Bool),
            },
        }
    }

    /// The left operand's type and the result type of `a or b` when `a` is
    /// nullable and `b` supplies its default: `T` for a plain fallback, `T?`
    /// for a nullable one so `a or b or c` chains. `None` means the operands
    /// are an ordinary bool pair.
    ///
    /// The fallback's own type decides which of the two it is checked against,
    /// because a `T?` fallback does not satisfy an expected `T` -- checking
    /// against `T` first would report a mismatch instead of chaining.
    fn nullable_or_types(
        &self,
        left: &Expr,
        right: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Option<(Type, Type)>, Diagnostic> {
        let Ok(nullable_ty @ Type::Nullable(_)) =
            self.infer_expr_type(left, types, None).map(first_of_multi)
        else {
            return Ok(None);
        };
        let inner = nullable_ty.nullable_inner().expect("matched Type::Nullable");
        // `bool?` is the one nullable type whose inner values include the value
        // a reader expects `or` to fall through on, so `flag or true` cannot be
        // read without knowing which rule applies.
        if inner == Type::Bool {
            return Err(Diagnostic::new(
                "'or' does not apply to a bool? left operand: whether a present \
                 'false' takes the fallback is ambiguous. Write 'flag == true' for \
                 set-and-true, or compare against nil explicitly",
            ));
        }
        let fallback_is_nullable = matches!(
            self.infer_expr_type(right, types, None).map(first_of_multi),
            Ok(Type::Nullable(_) | Type::Nil)
        );
        let result_ty = if fallback_is_nullable {
            nullable_ty.clone()
        } else {
            inner.clone()
        };
        self.infer_expr_type(right, types, Some(result_ty.clone()))
            .map_err(|_| {
                Diagnostic::new(format!(
                    "the fallback of 'or' must be {inner} or {nullable_ty} \
                     to match the nullable left operand"
                ))
            })?;
        Ok(Some((nullable_ty, result_ty)))
    }

    fn infer_binary_operand_type(
        &self,
        left: &Expr,
        right: &Expr,
        op: &BinaryOp,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        let expected_numeric = match expected {
            Some(Type::Numeric(numeric)) => Some(numeric),
            _ => None,
        };

        match op {
            BinaryOp::And | BinaryOp::Or => Ok(Type::Bool),
            BinaryOp::Eq | BinaryOp::NotEq => {
                if matches!(left, Expr::Nil(..)) || matches!(right, Expr::Nil(..)) {
                    let value = if matches!(left, Expr::Nil(..)) {
                        right
                    } else {
                        left
                    };
                    let value_ty =
                        first_of_multi(self.infer_expr_type(value, types, None)?);
                    if matches!(value_ty, Type::Nil | Type::Nullable(_)) {
                        return Ok(Type::Bool);
                    }
                    // An empty multi-value result (e.g. `string.byte` with a
                    // statically empty range) adjusts to nil in scalar
                    // context, so the comparison is well-typed.
                    if matches!(&value_ty, Type::Multi(parts) if parts.is_empty()) {
                        return Ok(Type::Bool);
                    }
                    return Err(Diagnostic::new(
                        "nil comparison requires a nullable operand",
                    ));
                }
                let left_ty = first_of_multi(self.infer_expr_type(left, types, None)?);
                if let Type::Nullable(_) = &left_ty {
                    return Ok(left_ty);
                }
                // An unknown operand compares with Lua dynamic equality; the
                // other side is boxed into unknown as needed.
                if left_ty == Type::Unknown {
                    return Ok(Type::Unknown);
                }
                if let Ok(probe_right) = self
                    .infer_expr_type(right, types, None)
                    .map(first_of_multi)
                {
                    if matches!(probe_right, Type::Nullable(_)) {
                        return Ok(probe_right);
                    }
                    if probe_right == Type::Unknown {
                        return Ok(Type::Unknown);
                    }
                }
                if left_ty == Type::Bool {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::Bool))?;
                    if right_ty == Type::Bool {
                        Ok(Type::Bool)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else if left_ty == Type::String {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::String))?;
                    if right_ty == Type::String {
                        Ok(Type::String)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else if left_ty == Type::Bytes {
                    let right_ty = self.infer_expr_type(right, types, Some(Type::Bytes))?;
                    if right_ty == Type::Bytes {
                        Ok(Type::Bytes)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else if matches!(left_ty, Type::TypedArray(_) | Type::Buffer) {
                    // Typed arrays and mutable buffers compare by identity.
                    let right_ty = self.infer_expr_type(right, types, Some(left_ty.clone()))?;
                    if right_ty == left_ty {
                        Ok(left_ty)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else if left_ty == Type::Extern {
                    // Extern references compare by host identity. Opaque extern
                    // aliases are already erased to `extern` at this stage.
                    let right_ty = first_of_multi(self.infer_expr_type(
                        right,
                        types,
                        Some(Type::Extern),
                    )?);
                    if right_ty == Type::Extern {
                        Ok(Type::Extern)
                    } else {
                        Err(Diagnostic::new(
                            "could not resolve operand type during IR lowering",
                        ))
                    }
                } else {
                    infer_numeric_common_type(
                        left,
                        right,
                        types,
                        expected_numeric,
                        |expr, expected| self.infer_expr_type(expr, types, expected),
                    )
                }
            }
            BinaryOp::Concat => {
                let left_ty = first_of_multi(self.infer_expr_type(left, types, None)?);
                let right_ty = first_of_multi(self.infer_expr_type(right, types, None)?);
                if left_ty == Type::Bytes || right_ty == Type::Bytes {
                    if left_ty == Type::Bytes && right_ty == Type::Bytes {
                        Ok(Type::Bytes)
                    } else {
                        Err(Diagnostic::new("could not resolve operand type during IR lowering"))
                    }
                } else if string_concat_operand_type(&left_ty)
                    && string_concat_operand_type(&right_ty)
                {
                    Ok(Type::String)
                } else {
                    Err(Diagnostic::new(
                        "could not resolve operand type during IR lowering",
                    ))
                }
            }
            BinaryOp::Add => {
                let left_ty = self.infer_expr_type(left, types, None)?;
                if left_ty == Type::String {
                    Err(Diagnostic::new(
                        "could not resolve operand type during IR lowering",
                    ))
                } else {
                    infer_numeric_common_type(
                        left,
                        right,
                        types,
                        expected_numeric,
                        |expr, expected| self.infer_expr_type(expr, types, expected),
                    )
                }
            }
            BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Pow
            | BinaryOp::Less
            | BinaryOp::LessEq
            | BinaryOp::Greater
            | BinaryOp::GreaterEq => {
                if matches!(
                    op,
                    BinaryOp::Less | BinaryOp::LessEq | BinaryOp::Greater | BinaryOp::GreaterEq
                ) {
                    let left_ty = self.infer_expr_type(left, types, None)?;
                    if left_ty == Type::Bytes {
                        let right_ty = self.infer_expr_type(right, types, Some(Type::Bytes))?;
                        if right_ty == Type::Bytes {
                            return Ok(Type::Bytes);
                        }
                    } else if left_ty == Type::String {
                        let right_ty = self.infer_expr_type(right, types, Some(Type::String))?;
                        if right_ty == Type::String {
                            return Ok(Type::String);
                        }
                    }
                }
                infer_numeric_common_type(left, right, types, expected_numeric, |expr, expected| {
                    self.infer_expr_type(expr, types, expected)
                })
            }
        }
    }

    fn coerce_value(
        &mut self,
        value: ValueId,
        actual: Type,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        match expected {
            None => Ok(value),
            Some(expected) if actual == expected => Ok(value),
            // A variadic pack in a scalar context contributes its first value.
            Some(expected)
                if matches!(actual, Type::Variadic(_))
                    && !matches!(expected, Type::Variadic(_)) =>
            {
                let Type::Variadic(element) = actual else {
                    unreachable!()
                };
                let zero = self.emit_i32_const(0);
                let first = self.emit(Instruction::ArrayGet {
                    array: value,
                    index: zero,
                    element_ty: (*element).clone(),
                });
                self.coerce_value(first, Arc::unwrap_or_clone(element), Some(expected))
            }
            // Multi-value adjustment: take the first value.
            Some(expected)
                if matches!(actual, Type::Multi(_)) && !matches!(expected, Type::Multi(_)) =>
            {
                let Type::Multi(mut parts) = actual else {
                    unreachable!()
                };
                if parts.is_empty() {
                    return Err(Diagnostic::new(
                        "cannot use an empty multi-value result as a value",
                    ));
                }
                let first_ty = parts.remove(0);
                let first = self.emit(Instruction::MultiGet {
                    value,
                    index: 0,
                    ty: first_ty.clone(),
                });
                self.coerce_value(first, first_ty, Some(expected))
            }
            // Multi-value adjustment at tuple width: read the values the
            // context asked for and repack, dropping the rest.
            Some(Type::Multi(expected_parts))
                if multi_truncates_to(&actual, &expected_parts) =>
            {
                let Type::Multi(actual_parts) = actual else {
                    unreachable!()
                };
                let mut values = Vec::with_capacity(expected_parts.len());
                let mut types = Vec::with_capacity(expected_parts.len());
                for (index, (actual_ty, expected_ty)) in
                    actual_parts.into_iter().zip(expected_parts).enumerate()
                {
                    let part = self.emit(Instruction::MultiGet {
                        value,
                        index,
                        ty: actual_ty.clone(),
                    });
                    types.push(coerce_type(actual_ty.clone(), Some(expected_ty.clone()))?);
                    values.push(self.coerce_value(part, actual_ty, Some(expected_ty))?);
                }
                Ok(self.emit(Instruction::PackMulti { values, types }))
            }
            // Nullable truthiness: non-nil is true.
            Some(Type::Bool) if matches!(actual, Type::Nullable(_)) => {
                let inner = actual.nullable_inner().expect("checked nullable");
                let is_null = self.emit(Instruction::IsNull { value, ty: inner });
                let false_value = self.emit(Instruction::Bool(false));
                Ok(self.emit(Instruction::Binary {
                    op: BinaryOp::Eq,
                    left: is_null,
                    right: false_value,
                    operand_ty: Type::Bool,
                    result_ty: Type::Bool,
                }))
            }
            Some(expected) => {
                let target = coerce_type(actual.clone(), Some(expected.clone()))?;
                if target == actual {
                    Ok(value)
                } else if target.nullable_inner() == Some(Type::Unknown) && actual != Type::Nil {
                    // Guarded recursive aliases use `unknown` as the finite
                    // runtime anchor. `unknown` is already a nullable anyref,
                    // so widening a concrete recursive record into `Node?`
                    // first boxes it to that anchor; there is no separate
                    // nullable wrapper (or numeric conversion) to emit.
                    if actual == Type::Unknown {
                        Ok(value)
                    } else {
                        Ok(self.emit(Instruction::Cast {
                            value,
                            from: to_runtime_type(&actual),
                            to: Type::Unknown,
                        }))
                    }
                } else if to_runtime_type(&target) == to_runtime_type(&actual) {
                    // The two annotations name the same runtime value — a tagged
                    // union against the canonical record that represents it, or a
                    // variant widening to its union. There is nothing to convert.
                    Ok(value)
                } else {
                    Ok(self.emit(Instruction::Cast {
                        value,
                        // The value's IR-level type is the canonical record for
                        // TaggedUnion/TaggedVariant; reflect that in the cast so it
                        // matches what `verify` infers for `value`.
                        from: to_runtime_type(&actual),
                        to: target,
                    }))
                }
            }
        }
    }

    fn coerce_method_receiver(
        &mut self,
        value: ValueId,
        actual: &Type,
        expected: &Type,
    ) -> Result<ValueId, Diagnostic> {
        if actual == expected {
            return Ok(value);
        }
        match (actual, expected) {
            (Type::Record(actual_fields), Type::Record(expected_fields))
                if expected_fields
                    .iter()
                    .all(|(name, expected_ty)| actual_fields.get(name) == Some(expected_ty)) =>
            {
                let fields = expected_fields
                    .iter()
                    .map(|(name, field_ty)| {
                        self.emit(Instruction::StructGet {
                            base: value,
                            field: name.clone(),
                            field_ty: field_ty.clone(),
                        })
                    })
                    .collect();
                Ok(self.emit(Instruction::StructNew {
                    struct_ty: expected.clone(),
                    fields,
                }))
            }
            _ => self.coerce_value(value, actual.clone(), Some(expected.clone())),
        }
    }

    fn write_back_method_receiver_mutations(
        &mut self,
        original: ValueId,
        projected: ValueId,
        actual: &Type,
        expected: &Type,
    ) -> Result<(), Diagnostic> {
        if actual == expected {
            return Ok(());
        }
        match (actual, expected) {
            (Type::Record(actual_fields), Type::Record(expected_fields))
                if expected_fields
                    .iter()
                    .all(|(name, expected_ty)| actual_fields.get(name) == Some(expected_ty)) =>
            {
                for (field, field_ty) in expected_fields.iter() {
                    let updated = self.emit(Instruction::StructGet {
                        base: projected,
                        field: field.clone(),
                        field_ty: field_ty.clone(),
                    });
                    self.emit(Instruction::StructSet {
                        base: original,
                        field: field.clone(),
                        value: updated,
                    });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn explicit_cast(
        &mut self,
        value: ValueId,
        from: Type,
        to: Type,
    ) -> Result<ValueId, Diagnostic> {
        require_numeric_cast(from.clone(), to.clone())?;
        if from == to {
            Ok(value)
        } else {
            Ok(self.emit(Instruction::Cast { value, from, to }))
        }
    }

    fn infer_array_literal_type(
        &self,
        elements: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Result<Type, Diagnostic> {
        // Typed-array literals allocate in linear memory; elements accept any
        // numeric type (converted on store, see infer_array_literal in
        // waluau-hir).
        if let Some(Type::TypedArray(kind)) = expected.as_ref() {
            let element_ty = Type::Numeric(kind.element_numeric_type());
            for element in elements {
                if matches!(element, Expr::Vararg(_)) {
                    return Err(Diagnostic::new(
                        "'...' is not supported in typed-array literals",
                    ));
                }
                let actual = self
                    .infer_expr_type(element, types, Some(element_ty.clone()))
                    .or_else(|_| self.infer_expr_type(element, types, None))
                    .map(first_of_multi)?;
                if !actual.is_numeric() {
                    return Err(Diagnostic::new(format!(
                        "{} element must be numeric, got {actual}",
                        kind.type_name()
                    )));
                }
            }
            return Ok(Type::TypedArray(*kind));
        }
        if elements.is_empty() {
            if let Some(element_ty) = expected.as_ref().and_then(Type::element_type) {
                return coerce_type(Type::Array(Arc::new(element_ty)), expected);
            }
            // `{}` with a record-like expectation is the empty record
            // literal, not an array literal (see infer_array_literal in
            // waluau-hir).
            if expected.as_ref().is_some_and(is_record_like) {
                return coerce_type(Type::record(BTreeMap::new()), expected);
            }
            return Err(inference_diagnostic(
                "inference/missing-context",
                DiagnosticCategory::MissingContext,
                "empty array literal requires explicit element type",
                "add an explicit element type annotation, e.g. local xs: {i32} = {}",
            ));
        }

        if elements.len() > 1 {
            for element in &elements[..elements.len() - 1] {
                if matches!(element, Expr::Vararg(_)) {
                    return Err(Diagnostic::new(
                        "'...' is only supported as the last element of a table constructor",
                    ));
                }
            }
        }
        let expected_element = expected.as_ref().and_then(Type::element_type);
        // Typed without an expectation, so a constructor that names a variant of
        // the expected element type cannot be inferred here; a failure only means
        // the element is not a variadic pack (see infer_array_literal in
        // waluau-hir).
        let trailing_pack = elements
            .last()
            .and_then(|element| self.infer_expr_type(element, types, None).ok())
            .is_some_and(|ty| match ty {
                Type::Variadic(_) => true,
                Type::Multi(parts) => matches!(parts.last(), Some(Type::Variadic(_))),
                _ => false,
            });
        // A trailing variadic expression splats unknown values into the array,
        // and an expected `{unknown}` boxes every element, so both force the
        // element type to unknown.
        if trailing_pack || expected_element == Some(Type::Unknown) {
            return coerce_type(Type::Array(Arc::new(Type::Unknown)), expected);
        }
        // A tagged union stays the element type of the whole literal rather than
        // narrowing to the first element's variant (see infer_array_literal in
        // waluau-hir), so every element resolves its constructor against all the
        // variants the array declares.
        if let Some(union_ty) = expected_element.as_ref().filter(|ty| {
            matches!(ty, Type::TaggedUnion(_))
                || matches!(ty, Type::Opaque { ty, .. } if matches!(ty.as_ref(), Type::TaggedUnion(_)))
        }) {
            return coerce_type(Type::Array(Arc::new(union_ty.clone())), expected);
        }
        let mut iter = elements.iter();
        let first = iter.next().expect("non-empty array literal");
        let mut element_ty = self.infer_expr_type(first, types, expected_element.clone())?;
        for element in iter {
            if element_ty == Type::Unknown {
                continue;
            }
            // Heterogeneous literals without a concrete expected element type
            // fall back to `{unknown}` with boxed elements (see
            // infer_array_literal in waluau-hir).
            match self
                .infer_expr_type(element, types, Some(element_ty.clone()))
                .and_then(|actual| common_element_type(element_ty.clone(), actual))
            {
                Ok(unified) => element_ty = unified,
                Err(error) => {
                    if expected_element.is_some() {
                        return Err(error);
                    }
                    element_ty = Type::Unknown;
                }
            }
        }

        coerce_type(Type::Array(Arc::new(element_ty)), expected)
    }

    fn lower_coroutine_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        let i32_ty = Type::Numeric(NumericType::I32);
        match name {
            COROUTINE_CREATE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CREATE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let callee_ty = Type::Function {
                    params: Vec::new(),
                    return_type: Arc::new(i32_ty.clone()),
                    has_self: false,
                };
                let coroutine_ty = match self.infer_expr_type(&args[0], &scope.types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                if coroutine_ty != callee_ty {
                    return Some(Err(Diagnostic::new(
                        "coroutine.create expects a zero-argument i32-returning function",
                    )));
                }
                let callee = match self.lower_expr(&args[0], scope, Some(callee_ty)) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let value = self.emit(Instruction::CoroutineCreate { callee });
                Some(self.coerce_value(value, Type::Thread, expected))
            }
            COROUTINE_RESUME => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_RESUME} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine = match self.lower_expr(&args[0], scope, Some(Type::Thread)) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                // When the expected type is a tagged union, emit the tagged-resume instruction
                // that returns a canonical `{ tag: i32, value: unknown }` record.
                if matches!(&expected, Some(Type::TaggedUnion(_)) | Some(Type::TaggedVariant(_))) {
                    let yielded_tag = match self.variant_tag_id("Yielded") {
                        Ok(tag) => tag,
                        Err(error) => return Some(Err(error)),
                    };
                    let finished_tag = match self.variant_tag_id("Finished") {
                        Ok(tag) => tag,
                        Err(error) => return Some(Err(error)),
                    };
                    let error_tag = match self.variant_tag_id("Error") {
                        Ok(tag) => tag,
                        Err(error) => return Some(Err(error)),
                    };
                    let value = self.emit(Instruction::CoroutineResumeTagged {
                        coroutine,
                        yielded_tag,
                        finished_tag,
                        error_tag,
                    });
                    // Return the canonical record value; the source-level TaggedUnion type is
                    // maintained by the caller via the explicit annotation in types[name].
                    return Some(Ok(value));
                }
                let value = self.emit(Instruction::CoroutineResume { coroutine });
                Some(self.coerce_value(
                    value,
                    Type::Multi(vec![Type::Bool, Type::Unknown]),
                    expected,
                ))
            }
            COROUTINE_AWAIT_PROMISE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_AWAIT_PROMISE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let promise_ty = match self.infer_expr_type(&args[0], &scope.types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                if !is_promise_like_extern(&promise_ty) {
                    return Some(Err(Diagnostic::new(
                        "coroutine.await_promise expects an extern Promise-like value",
                    )));
                }
                let promise = match self.lower_expr(&args[0], scope, Some(promise_ty)) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let resume_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::CoroutineAwaitPromise {
                        promise,
                        resume_block,
                    },
                );
                self.current_block = resume_block;
                let value = self.emit(Instruction::CoroutineAwaitResult);
                Some(self.coerce_value(value, Type::Unknown, expected))
            }
            COROUTINE_CLOSE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CLOSE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine = match self.lower_expr(&args[0], scope, Some(Type::Thread)) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let value = self.emit(Instruction::CoroutineClose { coroutine });
                Some(self.coerce_value(value, Type::Bool, expected))
            }
            COROUTINE_YIELD => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_YIELD} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let yield_value = match self.lower_expr(&args[0], scope, Some(Type::Unknown))
                {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let resume_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::CoroutineYield {
                        value: yield_value,
                        resume_block,
                    },
                );
                self.current_block = resume_block;
                let value = self.emit(Instruction::Unit);
                Some(self.coerce_value(value, Type::Unit, expected))
            }
            _ => None,
        }
    }

    fn lower_promise_await_value(
        &mut self,
        promise_expr: &Expr,
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let promise_ty = self.infer_expr_type(promise_expr, &scope.types, None)?;
        if !is_promise_like_extern(&promise_ty) {
            return Err(Diagnostic::new(
                "promise.await expects a Promise<T> extern value",
            ));
        }
        let promise = self.lower_expr(promise_expr, scope, Some(promise_ty))?;
        let resume_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::CoroutineAwaitPromise {
                promise,
                resume_block,
            },
        );
        self.current_block = resume_block;
        let value = self.emit(Instruction::CoroutineAwaitResult);
        match expected {
            Some(target) if target != Type::Unknown => Ok(self.emit(Instruction::Cast {
                value,
                from: Type::Unknown,
                to: target,
            })),
            other => self.coerce_value(value, Type::Unknown, other),
        }
    }

    fn lower_promise_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        match name {
            PROMISE_AWAIT => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{PROMISE_AWAIT} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                Some(self.lower_promise_await_value(&args[0], scope, expected))
            }
            _ => None,
        }
    }

    fn lower_pcall_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != PCALL {
            return None;
        }
        if args.is_empty() {
            return Some(Err(Diagnostic::new("{PCALL} expects at least 1 argument")));
        }
        let intrinsic = builtin_name(&args[0]).and_then(|intrinsic_name| {
            self.intrinsic_value_signature_for_call(
                &intrinsic_name,
                &args[1..],
                &scope.types,
            )
            .map(|signature| signature.map(|signature| (intrinsic_name, signature)))
        });
        let intrinsic = match intrinsic {
            Some(Ok(intrinsic)) => Some(intrinsic),
            Some(Err(error)) => return Some(Err(error)),
            None => None,
        };
        let host_import = if intrinsic.is_none() {
            match self.host_import_value_for_call(&args[0], &args[1..], &scope.types) {
                Ok(host_import) => host_import,
                Err(error) => return Some(Err(error)),
            }
        } else {
            None
        };
        let callee_ty = if let Some((_, (params, return_type))) = &intrinsic {
            Type::Function {
                params: params.clone(),
                return_type: Arc::new(return_type.clone()),
                has_self: false,
            }
        } else if let Some((_, _, params, return_type)) = &host_import {
            Type::Function {
                params: params.clone(),
                return_type: Arc::new(return_type.clone()),
                has_self: false,
            }
        } else {
            match self.infer_expr_type(&args[0], &scope.types, None) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            }
        };
        if callee_ty == Type::Unknown {
            let callee = match self.lower_expr(&args[0], scope, Some(Type::Unknown)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let packed_args = match self.lower_dynamic_pcall_args(&args[1..], scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let value = self.emit(Instruction::ProtectedCallUnknown {
                callee,
                args: packed_args,
            });
            return Some(self.coerce_value(
                value,
                Type::Multi(vec![Type::Bool, Type::Unknown]),
                expected,
            ));
        }
        let Type::Function {
            params,
            return_type,
            ..
        } = callee_ty.clone()
        else {
            return Some(Err(Diagnostic::new(format!(
                "{PCALL} expects a function, got {callee_ty}"
            ))));
        };
        if !call_arity_matches(&params, args.len() - 1) {
            return Some(Err(Diagnostic::new(format!(
                "{PCALL} protected function expects {} arguments, got {}",
                params.len(),
                args.len() - 1
            ))));
        }
        let callee = if let Some((intrinsic_name, (params, return_type))) = intrinsic {
            match self.lower_intrinsic_value(
                &intrinsic_name,
                params,
                return_type,
                Some(callee_ty.clone()),
            ) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            }
        } else if let Some((import_name, symbol_id, params, return_type)) = host_import {
            match self.lower_host_import_value(
                &import_name,
                symbol_id,
                params,
                return_type,
                Some(callee_ty.clone()),
            ) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            }
        } else {
            match self.lower_expr(&args[0], scope, Some(callee_ty)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            }
        };
        let lowered_args = match self.lower_fixed_call_args(&args[1..], &params, scope) {
            Ok(args) => args,
            Err(error) => return Some(Err(error)),
        };
        let return_type = Arc::unwrap_or_clone(return_type);
        let result_ty = Type::protected_call_result(&return_type);
        let value = self.emit(Instruction::ProtectedCall {
            callee,
            args: lowered_args,
            params,
            return_type,
        });
        Some(self.coerce_value(value, result_ty, expected))
    }

    /// Box all arguments to a dynamic `pcall` into one runtime-sized pack.
    /// A trailing `...` is appended in full, preserving the caller's runtime
    /// arity for the dynamic closure wrapper.
    fn lower_dynamic_pcall_args(
        &mut self,
        args: &[Expr],
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        if args.len() > 1 && args[..args.len() - 1].iter().any(|arg| matches!(arg, Expr::Vararg(_))) {
            return Err(Diagnostic::new(
                "'...' is only supported as the last argument of a call",
            ));
        }
        let trailing_vararg = matches!(args.last(), Some(Expr::Vararg(_)));
        let explicit = if trailing_vararg {
            &args[..args.len() - 1]
        } else {
            args
        };
        let elements = explicit
            .iter()
            .map(|arg| self.lower_expr(arg, scope, Some(Type::Unknown)))
            .collect::<Result<Vec<_>, _>>()?;
        let pack = self.emit(Instruction::ArrayNew {
            element_ty: Type::Unknown,
            elements,
        });
        if trailing_vararg {
            let forwarded = self
                .vararg_value
                .ok_or_else(|| Diagnostic::new("'...' used outside a vararg function"))?;
            self.emit_array_append_all(pack, forwarded);
        }
        Ok(pack)
    }

    fn lower_error_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != ERROR {
            return None;
        }
        if !(1..=2).contains(&args.len()) {
            return Some(Err(Diagnostic::new(format!(
                "{ERROR} expects 1 or 2 arguments, got {}",
                args.len()
            ))));
        }
        let message = match self.lower_expr(&args[0], scope, Some(Type::String)) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        if args.len() == 2 {
            if let Err(error) = self.lower_expr(
                &args[1],
                scope,
                Some(Type::Numeric(NumericType::I32)),
            ) {
                return Some(Err(error));
            }
        }
        let unit = self.emit(Instruction::Unit);
        self.emit(Instruction::Throw { error: message });
        self.set_terminator(self.current_block, Terminator::Unreachable { span: args[0].span() });
        self.current_block = DEAD_BLOCK;
        Some(self.coerce_value(unit, Type::Unit, expected))
    }

    fn lower_promise_await_method_call(
        &mut self,
        receiver: &Expr,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != "await" {
            return None;
        }
        if !args.is_empty() {
            return Some(Err(Diagnostic::new(format!(
                "Promise<T>:await expects 0 arguments, got {}",
                args.len()
            ))));
        }
        Some(self.lower_promise_await_value(receiver, scope, expected))
    }

    fn infer_coroutine_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let Expr::Call { args, .. } = call else {
            return None;
        };
        match name {
            COROUTINE_CREATE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CREATE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match &coroutine_ty {
                    Type::Function {
                        params,
                        return_type,
                        ..
                    } if params.is_empty() && **return_type == Type::Numeric(NumericType::I32) => {
                        Some(Ok(Type::Thread))
                    }
                    _ => Some(Err(Diagnostic::new(
                        "coroutine.create expects a zero-argument i32-returning function",
                    ))),
                }
            }
            COROUTINE_RESUME => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_RESUME} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match coroutine_ty {
                    Type::Thread => {
                        // Tagged-union expected type → result is canonical record (IR level).
                        if matches!(&expected, Some(Type::TaggedUnion(_)) | Some(Type::TaggedVariant(_))) {
                            return Some(Ok(Type::canonical_tagged_union_record()));
                        }
                        Some(Ok(Type::Multi(vec![Type::Bool, Type::Unknown])))
                    }
                    _ => Some(Err(Diagnostic::new("coroutine.resume expects a thread"))),
                }
            }
            COROUTINE_CLOSE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_CLOSE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let coroutine_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                match coroutine_ty {
                    Type::Thread => Some(Ok(Type::Bool)),
                    _ => Some(Err(Diagnostic::new("coroutine.close expects a thread"))),
                }
            }
            COROUTINE_AWAIT_PROMISE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_AWAIT_PROMISE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let promise_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                if is_promise_like_extern(&promise_ty) {
                    Some(Ok(Type::Unknown))
                } else {
                    Some(Err(Diagnostic::new(
                        "coroutine.await_promise expects an extern Promise-like value",
                    )))
                }
            }
            COROUTINE_YIELD => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{COROUTINE_YIELD} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let _ = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                Some(Ok(Type::Unit))
            }
            _ => None,
        }
    }

    fn infer_promise_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        match name {
            PROMISE_AWAIT => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{PROMISE_AWAIT} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let promise_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                if is_promise_like_extern(&promise_ty) {
                    Some(Ok(expected.unwrap_or(Type::Unknown)))
                } else {
                    Some(Err(Diagnostic::new(
                        "promise.await expects a Promise<T> extern value",
                    )))
                }
            }
            _ => None,
        }
    }

    fn infer_pcall_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != PCALL {
            return None;
        }
        if args.is_empty() {
            return Some(Err(Diagnostic::new("{PCALL} expects at least 1 argument")));
        }
        let intrinsic = builtin_name(&args[0]).and_then(|intrinsic_name| {
            self.intrinsic_value_signature_for_call(&intrinsic_name, &args[1..], types)
                .map(|signature| signature.map(|signature| (intrinsic_name, signature)))
        });
        let intrinsic = match intrinsic {
            Some(Ok(intrinsic)) => Some(intrinsic),
            Some(Err(error)) => return Some(Err(error)),
            None => None,
        };
        let host_import = if intrinsic.is_none() {
            match self.host_import_value_for_call(&args[0], &args[1..], types) {
                Ok(host_import) => host_import,
                Err(error) => return Some(Err(error)),
            }
        } else {
            None
        };
        let callee_ty = if let Some((_, (params, return_type))) = &intrinsic {
            Type::Function {
                params: params.clone(),
                return_type: Arc::new(return_type.clone()),
                has_self: false,
            }
        } else if let Some((_, _, params, return_type)) = &host_import {
            Type::Function {
                params: params.clone(),
                return_type: Arc::new(return_type.clone()),
                has_self: false,
            }
        } else {
            match self.infer_expr_type(&args[0], types, None) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            }
        };
        if callee_ty == Type::Unknown {
            for arg in args.iter().skip(1) {
                if let Err(error) = self.infer_expr_type(arg, types, None) {
                    return Some(Err(error));
                }
            }
            return Some(coerce_type(
                Type::Multi(vec![Type::Bool, Type::Unknown]),
                expected,
            ));
        }
        let Type::Function {
            params,
            return_type,
            ..
        } = callee_ty.clone()
        else {
            return Some(Err(Diagnostic::new(format!(
                "{PCALL} expects a function, got {callee_ty}"
            ))));
        };
        if !call_arity_matches(&params, args.len() - 1) {
            return Some(Err(Diagnostic::new(format!(
                "{PCALL} protected function expects {} arguments, got {}",
                params.len(),
                args.len() - 1
            ))));
        }
        for (arg, param_ty) in args.iter().skip(1).zip(params.iter()) {
            if let Err(error) = self
                .infer_expr_type(arg, types, Some(param_ty.clone()))
                .and_then(|actual| coerce_type(actual, Some(param_ty.clone())))
            {
                return Some(Err(error));
            }
        }
        Some(coerce_type(
            Type::protected_call_result(&return_type),
            expected,
        ))
    }

    fn infer_error_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != ERROR {
            return None;
        }
        if !(1..=2).contains(&args.len()) {
            return Some(Err(Diagnostic::new(format!(
                "{ERROR} expects 1 or 2 arguments, got {}",
                args.len()
            ))));
        }
        if let Err(error) = self.infer_expr_type(&args[0], types, Some(Type::String)) {
            return Some(Err(error));
        }
        if args.len() == 2 {
            if let Err(error) =
                self.infer_expr_type(&args[1], types, Some(Type::Numeric(NumericType::I32)))
            {
                return Some(Err(error));
            }
        }
        Some(Ok(expected.unwrap_or(Type::Unit)))
    }

    fn infer_promise_await_method_call_type(
        &self,
        name: &str,
        args: &[Expr],
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != "await" {
            return None;
        }
        if !args.is_empty() {
            return Some(Err(Diagnostic::new(format!(
                "Promise<T>:await expects 0 arguments, got {}",
                args.len()
            ))));
        }
        Some(Ok(expected.unwrap_or(Type::Unknown)))
    }

    fn lower_bit32_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        let (intrinsic, arity, result_ty) = match name {
            BIT32_BNOT => (BitwiseIntrinsic::Not, Some(1), Type::Numeric(NumericType::U32)),
            BIT32_BAND => (BitwiseIntrinsic::And, None, Type::Numeric(NumericType::U32)),
            BIT32_BOR => (BitwiseIntrinsic::Or, None, Type::Numeric(NumericType::U32)),
            BIT32_BXOR => (BitwiseIntrinsic::Xor, None, Type::Numeric(NumericType::U32)),
            BIT32_BTEST => (BitwiseIntrinsic::Test, None, Type::Bool),
            BIT32_LROTATE => (BitwiseIntrinsic::LRotate, Some(2), Type::Numeric(NumericType::U32)),
            BIT32_RROTATE => (BitwiseIntrinsic::RRotate, Some(2), Type::Numeric(NumericType::U32)),
            BIT32_LSHIFT => (BitwiseIntrinsic::LShift, Some(2), Type::Numeric(NumericType::U32)),
            BIT32_RSHIFT => (BitwiseIntrinsic::RShift, Some(2), Type::Numeric(NumericType::U32)),
            BIT32_ARSHIFT => {
                (BitwiseIntrinsic::ArithmeticRShift, Some(2), Type::Numeric(NumericType::U32))
            }
            BIT32_EXTRACT => {
                if !(2..=3).contains(&args.len()) {
                    return Some(Err(Diagnostic::new(format!(
                        "{name} expects 2 or 3 arguments, got {}",
                        args.len()
                    ))));
                }
                (BitwiseIntrinsic::Extract, None, Type::Numeric(NumericType::U32))
            }
            BIT32_REPLACE => {
                if !(3..=4).contains(&args.len()) {
                    return Some(Err(Diagnostic::new(format!(
                        "{name} expects 3 or 4 arguments, got {}",
                        args.len()
                    ))));
                }
                (BitwiseIntrinsic::Replace, None, Type::Numeric(NumericType::U32))
            }
            BIT32_BYTESWAP => {
                (BitwiseIntrinsic::ByteSwap, Some(1), Type::Numeric(NumericType::U32))
            }
            BIT32_COUNTLZ => {
                (BitwiseIntrinsic::CountLeadingZeros, Some(1), Type::Numeric(NumericType::U32))
            }
            BIT32_COUNTRZ => {
                (BitwiseIntrinsic::CountTrailingZeros, Some(1), Type::Numeric(NumericType::U32))
            }
            _ => return None,
        };
        if let Some(arity) = arity {
            if args.len() != arity {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects {arity} argument{}, got {}",
                    if arity == 1 { "" } else { "s" },
                    args.len()
                ))));
            }
        }

        let u32_ty = Type::Numeric(NumericType::U32);
        let i32_ty = Type::Numeric(NumericType::I32);
        let mut lowered = Vec::with_capacity(args.len().max(match intrinsic {
            BitwiseIntrinsic::Extract => 3,
            BitwiseIntrinsic::Replace => 4,
            _ => args.len(),
        }));
        for (index, arg) in args.iter().enumerate() {
            let signed_position = matches!(
                intrinsic,
                BitwiseIntrinsic::LRotate
                    | BitwiseIntrinsic::RRotate
                    | BitwiseIntrinsic::LShift
                    | BitwiseIntrinsic::RShift
                    | BitwiseIntrinsic::ArithmeticRShift
            ) && index == 1
                || intrinsic == BitwiseIntrinsic::Extract && index >= 1
                || intrinsic == BitwiseIntrinsic::Replace && index >= 2;
            let expected_arg = if signed_position { i32_ty.clone() } else { u32_ty.clone() };
            match self.lower_bit32_operand(name, index, arg, expected_arg, scope) {
                Ok(value) => lowered.push(value),
                Err(error) => return Some(Err(error)),
            }
        }
        if intrinsic == BitwiseIntrinsic::Extract && lowered.len() == 2
            || intrinsic == BitwiseIntrinsic::Replace && lowered.len() == 3
        {
            lowered.push(self.emit(Instruction::Number {
                ty: NumericType::I32,
                literal: NumberLiteral { raw: "1".to_string() },
            }));
        }

        let value = self.emit(Instruction::BitwiseIntrinsic {
            intrinsic,
            args: lowered,
            result_ty: result_ty.clone(),
        });
        Some(self.coerce_value(value, result_ty, expected))
    }

    /// Lowers one `bit32` argument. An operand that already has the exact
    /// `u32`/`i32` shape keeps the direct intrinsic path; any other number —
    /// including one that only becomes a number at runtime through `unknown` —
    /// converts through `f64` and Luau's modulo-2^32 argument rule.
    fn lower_bit32_operand(
        &mut self,
        name: &str,
        index: usize,
        arg: &Expr,
        expected: Type,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        let typed = self.infer_expr_type(arg, &scope.types, Some(expected.clone()));
        let natural = self
            .infer_expr_type(arg, &scope.types, None)
            .ok()
            .map(first_of_multi);
        // A dynamic operand can satisfy the typed path — `unknown` unboxes to
        // `u32`, and an f64 expression accepts a 32-bit expectation — but both
        // then reject the out-of-range and negative numbers Luau truncates, so
        // only genuine 32-bit values and literals stay on the direct path.
        let dynamic = natural.as_ref().is_some_and(bit32_dynamic_operand)
            && !bit32_direct_literal(arg, &expected);
        if !dynamic && matches!(&typed, Ok(ty) if ty == &expected) {
            return self.lower_expr(arg, scope, Some(expected));
        }
        let Some(dynamic_ty) = natural.filter(bit32_dynamic_operand) else {
            return Err(bit32_operand_mismatch(name, index, &expected, typed));
        };
        let value = self.lower_expr(arg, scope, Some(dynamic_ty.clone()))?;
        let number =
            self.coerce_value(value, dynamic_ty, Some(Type::Numeric(NumericType::F64)))?;
        Ok(self.emit(Instruction::BitwiseIntrinsic {
            intrinsic: BitwiseIntrinsic::TruncateToInt32,
            args: vec![number],
            result_ty: expected,
        }))
    }

    fn lower_tostring_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != TO_STRING {
            return None;
        }
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let value = match self.lower_tostring_operand(TO_STRING, &args[0], scope) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        Some(self.coerce_value(value, Type::String, expected))
    }

    /// Lowers one expression to its `tostring` representation, following the
    /// builtin's rules: `nil` folds to "nil", strings pass through, and every
    /// other supported type stringifies via `ToString`.
    fn lower_tostring_operand(
        &mut self,
        builtin: &str,
        arg: &Expr,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        let arg_ty = self.infer_expr_type(arg, &scope.types, None)?;
        if !tostring_supported_type(&arg_ty) {
            return Err(Diagnostic::new(format!(
                "{builtin} cannot convert a {arg_ty} value",
            )));
        }
        if arg_ty == Type::Nil {
            return Ok(self.emit(Instruction::String("nil".to_string())));
        }
        let lowered = self.lower_expr(arg, scope, Some(arg_ty.clone()))?;
        Ok(self.tostring_value(lowered, arg_ty))
    }

    /// Stringifies an already-lowered value of a `tostring`-supported,
    /// non-`nil` type.
    fn tostring_value(&mut self, value: ValueId, from: Type) -> ValueId {
        if from == Type::String {
            value
        } else if matches!(from, Type::Nullable(_)) {
            // A nullable value stringifies through the dynamic chain: nil
            // becomes "nil", the inner value formats as itself.
            let boxed = self.emit(Instruction::Cast {
                value,
                from,
                to: Type::Unknown,
            });
            self.emit(Instruction::ToString {
                value: boxed,
                from: Type::Unknown,
            })
        } else {
            self.emit(Instruction::ToString { value, from })
        }
    }

    fn lower_string_concat_operand(
        &mut self,
        expr: &Expr,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        let actual = first_of_multi(self.infer_expr_type(expr, &scope.types, None)?);
        if actual.is_numeric() {
            let value = self.lower_expr(expr, scope, Some(actual.clone()))?;
            Ok(self.emit(Instruction::ToString {
                value,
                from: actual,
            }))
        } else {
            self.lower_expr(expr, scope, Some(Type::String))
        }
    }

    fn lower_compound_rhs(
        &mut self,
        op: BinaryOp,
        expr: &Expr,
        target_ty: &Type,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        if op == BinaryOp::Concat && target_ty == &Type::String {
            self.lower_string_concat_operand(expr, scope)
        } else {
            self.lower_expr(expr, scope, Some(target_ty.clone()))
        }
    }

    fn lower_type_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if !matches!(name, TYPE | TYPEOF) {
            return None;
        }
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let arg_ty = match self.infer_expr_type(&args[0], &scope.types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        // `type(f())` inspects the single value `f()` adjusts to, so collapse a
        // multi-value result before classifying it (an empty result is `nil`).
        let arg_ty = adjust_to_single_value(arg_ty);
        let value = if arg_ty == Type::Unknown {
            let lowered = match self.lower_expr(&args[0], scope, Some(Type::Unknown)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            self.emit(Instruction::TypeName {
                value: lowered,
                from: Type::Unknown,
            })
        } else if matches!(arg_ty, Type::Nullable(_)) {
            // A nullable value's type is only known at runtime ("nil" vs the
            // inner type's name); classify through the dynamic chain.
            let lowered = match self.lower_expr(&args[0], scope, Some(arg_ty.clone())) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let boxed = self.emit(Instruction::Cast {
                value: lowered,
                from: arg_ty,
                to: Type::Unknown,
            });
            self.emit(Instruction::TypeName {
                value: boxed,
                from: Type::Unknown,
            })
        } else {
            let Some(type_name) = lua_type_name(&arg_ty) else {
                return Some(Err(Diagnostic::new(format!(
                    "{name} cannot classify a {arg_ty} value at compile time; \
                     annotate the argument with a concrete type or `unknown` \
                    to classify it at runtime",
                ))));
            };
            // The result can be folded from the static type, but Lua still
            // evaluates the argument exactly once before classifying it.
            // Lower without an expected type because the value is discarded;
            // this also preserves calls that return an empty multi-value pack,
            // which adjusts to nil for type()/typeof().
            if let Err(error) = self.lower_expr(&args[0], scope, None) {
                return Some(Err(error));
            }
            self.emit(Instruction::String(type_name.to_string()))
        };
        Some(self.coerce_value(value, Type::String, expected))
    }

    fn lower_tonumber_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != TO_NUMBER {
            return None;
        }
        if args.is_empty() || args.len() > 2 {
            return Some(Err(Diagnostic::new(format!(
                "{TO_NUMBER} expects 1 or 2 arguments, got {}",
                args.len()
            ))));
        }
        let arg_ty = match self.infer_expr_type(&args[0], &scope.types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if !(arg_ty.is_numeric() || arg_ty == Type::String || arg_ty == Type::Unknown) {
            return Some(Err(Diagnostic::new(format!(
                "{TO_NUMBER} expects a numeric, string, or unknown argument, got {arg_ty}",
            ))));
        }
        if args.len() == 2 && !(arg_ty == Type::String || arg_ty == Type::Unknown) {
            return Some(Err(Diagnostic::new(format!(
                "{TO_NUMBER} with a base expects a string or unknown value, got {arg_ty}",
            ))));
        }
        let result_ty = Type::Numeric(NumericType::F64);
        if args.len() == 1 && arg_ty.is_numeric() {
            let lowered = match self.lower_expr(&args[0], scope, Some(arg_ty.clone())) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let value = if arg_ty == result_ty {
                lowered
            } else {
                self.emit(Instruction::Cast {
                    value: lowered,
                    from: arg_ty.clone(),
                    to: result_ty.clone(),
                })
            };
            return Some(self.coerce_value(value, result_ty, expected));
        }
        let lowered = match self.lower_expr(&args[0], scope, Some(arg_ty.clone())) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let base = if let Some(base) = args.get(1) {
            match self.lower_expr(base, scope, Some(Type::Numeric(NumericType::I32))) {
                Ok(value) => Some(value),
                Err(error) => return Some(Err(error)),
            }
        } else {
            None
        };
        // Runtime parses can fail, so the result is `f64?` (nil on failure).
        // A numeric expected type keeps the plain f64 result (the unbox traps
        // on nil, matching Lua's arithmetic-on-nil error).
        let nullable_ty = Type::Nullable(Arc::new(result_ty.clone()));
        let value = self.emit(Instruction::ToNumber {
            value: lowered,
            from: arg_ty,
            base,
        });
        if string_byte_requires_numeric_value(expected.as_ref()) {
            let value = self.emit(Instruction::Cast {
                value,
                from: nullable_ty,
                to: result_ty.clone(),
            });
            Some(self.coerce_value(value, result_ty, expected))
        } else {
            Some(self.coerce_value(value, nullable_ty, expected))
        }
    }

    fn lower_select_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != SELECT {
            return None;
        }
        if args.len() != 2 {
            return Some(Err(Diagnostic::new(format!(
                "{SELECT} expects 2 arguments, got {}",
                args.len()
            ))));
        }
        let count_marker = matches!(&args[0], Expr::String(marker, _) if marker == "#");
        // First, infer the type to validate it's an array
        let arg_ty = match self.infer_expr_type(&args[1], &scope.types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };

        if count_marker && arg_ty == Type::Unit {
            // Preserve the side effects of the zero-result expression before
            // materializing the result-pack count.
            if let Err(error) = self.lower_expr(&args[1], scope, Some(Type::Unit)) {
                return Some(Err(error));
            }
            let value = self.emit_i32_const(0);
            return Some(self.coerce_value(value, Type::Numeric(NumericType::I32), expected));
        }

        let Some(element_ty) = arg_ty.element_type() else {
            return Some(Err(Diagnostic::new(format!(
                "{SELECT} expects an array, got {arg_ty}"
            ))));
        };

        let arg_is_pack = matches!(arg_ty, Type::Variadic(_));
        // Now lower the expression with the known array type
        let array = match self.lower_expr(&args[1], scope, Some(arg_ty)) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        if count_marker {
            let value = self.emit(Instruction::ArrayLen { array });
            return Some(self.coerce_value(value, Type::Numeric(NumericType::I32), expected));
        }
        // `select(n, ...)` returns the single value at 1-based position n;
        // negative n counts from the end (`len + n`). Out-of-range positions
        // trap on the array bounds check where Lua raises "index out of
        // range".
        let n = match self.lower_expr(&args[0], scope, Some(Type::Numeric(NumericType::I32)))
        {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let i32_ty = Type::Numeric(NumericType::I32);
        let zero = self.emit_i32_const(0);
        let one = self.emit_i32_const(1);
        let positive = self.emit(Instruction::Binary {
            op: BinaryOp::Greater,
            left: n,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        let preheader = self.current_block;
        let from_front = self.new_block();
        let from_back = self.new_block();
        let merge = self.new_block();
        self.set_terminator(
            preheader,
            Terminator::Branch {
                condition: positive,
                then_block: from_front,
                else_block: from_back,
            },
        );
        self.current_block = from_front;
        let front_index = self.emit(Instruction::Binary {
            op: BinaryOp::Sub,
            left: n,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty.clone(),
        });
        self.set_terminator(from_front, Terminator::Jump(merge));
        self.current_block = from_back;
        let len = self.emit(Instruction::ArrayLen { array });
        let back_index = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: len,
            right: n,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty,
        });
        self.set_terminator(from_back, Terminator::Jump(merge));
        self.current_block = merge;
        let index = self.emit(Instruction::Phi(vec![
            (from_front, front_index),
            (from_back, back_index),
        ]));
        // A variadic pack stores boxed elements whatever its static element
        // annotation says, so the get runs at `unknown` and the result casts
        // to the annotated element type.
        let runtime_element = if arg_is_pack {
            Type::Unknown
        } else {
            element_ty.clone()
        };
        let value = self.emit(Instruction::ArrayGet {
            array,
            index,
            element_ty: runtime_element.clone(),
        });
        let value = match self.coerce_value(value, runtime_element, Some(element_ty.clone())) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        Some(self.coerce_value(value, element_ty, expected))
    }

    /// Shape the argument list for a call to a vararg function, whose IR-level
    /// signature ends with the implicit `Array(Unknown)` vararg slot.
    ///
    /// Explicit arguments fill the callee's fixed parameters first; leftovers
    /// are boxed into a fresh tail array. A trailing `...` forwards the
    /// caller's varargs: missing fixed parameters are drawn from the front of
    /// the forwarded varargs (trapping when too few values were passed, where
    /// Lua would see nil) and the rest become the callee's varargs. A final
    /// statically sized multi-value expression similarly fills remaining fixed
    /// parameters, then contributes each surplus result to the vararg tail.
    fn shape_vararg_call_args(
        &mut self,
        param_types: &[Type],
        args: &[Expr],
        scope: &Scope,
    ) -> Result<Vec<ValueId>, Diagnostic> {
        let fixed_count = param_types.len() - 1;
        if args.len() > 1 {
            for arg in &args[..args.len() - 1] {
                if matches!(arg, Expr::Vararg(_)) {
                    return Err(Diagnostic::new(
                        "'...' is only supported as the last argument of a call",
                    ));
                }
            }
        }
        let trailing_tail_ty = args
            .last()
            .map(|arg| self.infer_expr_type(arg, &scope.types, None))
            .transpose()?;
        let has_expanding_tail = trailing_tail_ty.as_ref().is_some_and(|ty| match ty {
            Type::Variadic(_) => true,
            Type::Multi(parts) => !parts.is_empty(),
            _ => false,
        });
        if has_expanding_tail {
            let tail_expr = args
                .last()
                .expect("expanding tail requires a trailing expression");
            let mut known = Vec::<(ValueId, Type)>::new();
            for (index, arg) in args[..args.len() - 1].iter().enumerate() {
                let target = param_types
                    .get(index)
                    .filter(|_| index < fixed_count)
                    .cloned()
                    .unwrap_or(Type::Unknown);
                let value = self.lower_expr(arg, scope, Some(target.clone()))?;
                known.push((value, target));
            }

            let pack = match trailing_tail_ty.expect("checked above") {
                Type::Variadic(element_ty) => Some(self.lower_expr(
                    tail_expr,
                    scope,
                    Some(Type::Variadic(element_ty)),
                )?),
                Type::Multi(parts) => {
                    let tuple = self.lower_expr(
                        tail_expr,
                        scope,
                        Some(Type::Multi(parts.clone())),
                    )?;
                    let fixed_len = if parts
                        .last()
                        .is_some_and(|part| matches!(part, Type::Variadic(_)))
                    {
                        parts.len() - 1
                    } else {
                        parts.len()
                    };
                    for (index, part) in parts[..fixed_len].iter().enumerate() {
                        let value = self.emit(Instruction::MultiGet {
                            value: tuple,
                            index,
                            ty: part.clone(),
                        });
                        known.push((value, part.clone()));
                    }
                    match parts.last() {
                        Some(Type::Variadic(element_ty)) => {
                            Some(self.emit(Instruction::MultiGet {
                                value: tuple,
                                index: parts.len() - 1,
                                ty: Type::Variadic(element_ty.clone()),
                            }))
                        }
                        _ => None,
                    }
                }
                _ => unreachable!("checked expanding tail above"),
            };

            let mut lowered = Vec::with_capacity(param_types.len());
            for (index, (value, actual)) in known.iter().take(fixed_count).enumerate() {
                lowered.push(self.coerce_value(
                    *value,
                    actual.clone(),
                    Some(param_types[index].clone()),
                )?);
            }
            if known.len() < fixed_count {
                if let Some(pack) = pack {
                    for (index, param_ty) in param_types
                        .iter()
                        .take(fixed_count)
                        .enumerate()
                        .skip(known.len())
                    {
                        let offset = self.emit_i32_const((index - known.len()) as i32);
                        let value = self.emit(Instruction::ArrayGet {
                            array: pack,
                            index: offset,
                            element_ty: Type::Unknown,
                        });
                        lowered.push(self.coerce_value(
                            value,
                            Type::Unknown,
                            Some(param_ty.clone()),
                        )?);
                    }
                    let start = self.emit_i32_const((fixed_count - known.len()) as i32);
                    let tail = self.emit(Instruction::ArraySlice {
                        array: pack,
                        start,
                        element_ty: Type::Unknown,
                    });
                    lowered.push(tail);
                } else {
                    for param_ty in param_types
                        .iter()
                        .take(fixed_count)
                        .skip(known.len())
                    {
                        lowered.push(self.emit_omitted_nullable_arg(param_ty)?);
                    }
                    lowered.push(self.emit(Instruction::ArrayNew {
                        element_ty: Type::Unknown,
                        elements: Vec::new(),
                    }));
                }
            } else {
                let extras = known
                    .into_iter()
                    .skip(fixed_count)
                    .map(|(value, actual)| {
                        self.coerce_value(value, actual, Some(Type::Unknown))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let tail = self.emit(Instruction::ArrayNew {
                    element_ty: Type::Unknown,
                    elements: extras,
                });
                if let Some(pack) = pack {
                    self.emit_array_append_all(tail, pack);
                }
                lowered.push(tail);
            }
            return Ok(lowered);
        }
        let trailing_vararg = matches!(args.last(), Some(Expr::Vararg(_)));
        let explicit = if trailing_vararg {
            &args[..args.len() - 1]
        } else {
            args
        };
        if !trailing_vararg
            && explicit.len() < required_param_count(&param_types[..fixed_count])
        {
            return Err(Diagnostic::new(format!(
                "function expects at least {} arguments, got {}",
                required_param_count(&param_types[..fixed_count]),
                explicit.len()
            )));
        }
        let mut lowered = Vec::with_capacity(param_types.len());
        for (arg, param_ty) in explicit.iter().zip(param_types.iter().take(fixed_count)) {
            lowered.push(self.lower_expr(arg, scope, Some(param_ty.clone()))?);
        }
        if !trailing_vararg {
            for param_ty in &param_types[lowered.len()..fixed_count] {
                lowered.push(self.emit_omitted_nullable_arg(param_ty)?);
            }
        }
        let mut extras = Vec::new();
        for arg in explicit.iter().skip(fixed_count) {
            extras.push(self.lower_expr(arg, scope, Some(Type::Unknown))?);
        }
        if !trailing_vararg {
            let tail = self.emit(Instruction::ArrayNew {
                element_ty: Type::Unknown,
                elements: extras,
            });
            lowered.push(tail);
            return Ok(lowered);
        }
        let varargs = self
            .vararg_value
            .ok_or_else(|| Diagnostic::new("'...' used outside a vararg function"))?;
        if explicit.len() < fixed_count {
            // Fill the remaining fixed parameters from the front of the
            // forwarded varargs, then pass the rest along as the callee's
            // varargs.
            let missing = fixed_count - explicit.len();
            for offset in 0..missing {
                let index = self.emit_i32_const(offset as i32);
                let value = self.emit(Instruction::ArrayGet {
                    array: varargs,
                    index,
                    element_ty: Type::Unknown,
                });
                let param_ty = param_types[explicit.len() + offset].clone();
                lowered.push(self.coerce_value(value, Type::Unknown, Some(param_ty))?);
            }
            let start = self.emit_i32_const(missing as i32);
            let tail = self.emit(Instruction::ArraySlice {
                array: varargs,
                start,
                element_ty: Type::Unknown,
            });
            lowered.push(tail);
        } else {
            // Every fixed parameter is covered by an explicit argument; the
            // callee's varargs are the leftover explicit values followed by a
            // copy of the caller's forwarded varargs.
            let tail = self.emit(Instruction::ArrayNew {
                element_ty: Type::Unknown,
                elements: extras,
            });
            self.emit_array_append_all(tail, varargs);
            lowered.push(tail);
        }
        Ok(lowered)
    }

    /// Lower an array index expression to i32. Non-i32 numeric indices are
    /// converted with a truncating cast (traps on NaN/out-of-range, where Lua
    /// raises an error), so f64-typed values — e.g. numeric for-loop
    /// variables over literal bounds — can index arrays.
    fn lower_index_to_i32(
        &mut self,
        index: &Expr,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        if !matches!(index, Expr::Number(..)) {
            if let Ok(index_ty) = self.infer_expr_type(index, &scope.types, None).map(first_of_multi) {
                if index_ty.is_numeric() && index_ty != i32_ty {
                    let value = self.lower_expr(index, scope, Some(index_ty.clone()))?;
                    return Ok(self.emit(Instruction::Cast {
                        value,
                        from: index_ty,
                        to: i32_ty,
                    }));
                }
            }
        }
        self.lower_expr(index, scope, Some(i32_ty))
    }

    /// Convert a 1-based authored ordinary-array index to the 0-based physical
    /// index used by growable Wasm GC array storage. Bytes and linear-memory
    /// typed arrays deliberately bypass this helper and remain 0-based.
    fn lower_authored_array_index(
        &mut self,
        index: &Expr,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        let authored = self.lower_index_to_i32(index, scope)?;
        let one = self.emit_i32_const(1);
        Ok(self.emit(Instruction::Binary {
            op: BinaryOp::Sub,
            left: authored,
            right: one,
            operand_ty: Type::Numeric(NumericType::I32),
            result_ty: Type::Numeric(NumericType::I32),
        }))
    }

    /// Lower a numeric expression, inserting an explicit-cast-style conversion
    /// when its own type differs from `target` (mirrors `lower_index_to_i32`).
    /// Typed-array element writes use this so any number converts on store,
    /// like JS typed arrays.
    fn lower_numeric_to(
        &mut self,
        expr: &Expr,
        target: &Type,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        if !matches!(expr, Expr::Number(..)) {
            if let Ok(actual) = self.infer_expr_type(expr, &scope.types, None).map(first_of_multi) {
                if actual.is_numeric() && &actual != target {
                    let value = self.lower_expr(expr, scope, Some(actual.clone()))?;
                    return Ok(self.emit(Instruction::Cast {
                        value,
                        from: actual,
                        to: target.clone(),
                    }));
                }
            }
        }
        self.lower_expr(expr, scope, Some(target.clone()))
    }

    /// Lower one numeric-operator operand. A statically `unknown` source is
    /// narrowed by `DynNumber`, which accepts both Lua number boxes and raises
    /// a catchable Lua error for every non-number. Statically typed operands
    /// retain the existing numeric lowering and coercion path.
    fn lower_checked_numeric_operand(
        &mut self,
        expr: &Expr,
        target: &Type,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        let actual = first_of_multi(self.infer_expr_type(expr, &scope.types, None)?);
        if actual == Type::Unknown {
            let boxed = self.lower_expr(expr, scope, Some(Type::Unknown))?;
            let number = self.emit(Instruction::DynNumber { value: boxed });
            let number_ty = Type::Numeric(NumericType::F64);
            return self.coerce_value(number, number_ty, Some(target.clone()));
        }
        self.lower_expr(expr, scope, Some(target.clone()))
    }

    /// Lower a typed-array literal. Fully constant literals become a
    /// `BufferConst` backed by a passive data segment; anything else
    /// allocates and stores each element at runtime.
    fn lower_typed_array_literal(
        &mut self,
        kind: TypedArrayKind,
        elements: &[Expr],
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        for element in elements {
            if matches!(element, Expr::Vararg(_)) {
                return Err(Diagnostic::new(
                    "'...' is not supported in typed-array literals",
                ));
            }
        }
        if let Some(bytes) = const_typed_array_bytes(kind, elements) {
            return Ok(self.emit(Instruction::BufferConst { kind, bytes }));
        }
        let element_ty = Type::Numeric(kind.element_numeric_type());
        let lowered = elements
            .iter()
            .map(|element| self.lower_numeric_to(element, &element_ty, scope))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.emit(Instruction::BufferNew {
            kind,
            elements: lowered,
        }))
    }

    fn lower_typed_array_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        let kind = typed_array_create_kind(name)?;
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let len = match self.lower_index_to_i32(&args[0], scope) {
            Ok(len) => len,
            Err(error) => return Some(Err(error)),
        };
        let value = self.emit(Instruction::BufferNewSized { kind, len });
        Some(self.coerce_value(value, Type::TypedArray(kind), expected))
    }

    fn lower_luau_buffer_i32(
        &mut self,
        expr: &Expr,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        // Untyped function parameters arrive as boxed Luau numbers (f64).
        // Convert through i64 so values through 2^32 can retain their low
        // bits instead of trapping in a direct f64-to-i32 conversion.
        let f64_ty = Type::Numeric(NumericType::F64);
        let i32_ty = Type::Numeric(NumericType::I32);
        let i64_ty = Type::Numeric(NumericType::I64);
        let number = self.lower_numeric_to(expr, &f64_ty, scope)?;
        let wide = self.emit(Instruction::Cast {
            value: number,
            from: f64_ty,
            to: i64_ty.clone(),
        });
        Ok(self.emit(Instruction::Cast {
            value: wide,
            from: i64_ty,
            to: i32_ty,
        }))
    }

    fn lower_buffer_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        let member = name.strip_prefix("buffer.")?;
        if member == "create" {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let len = match self.lower_luau_buffer_i32(&args[0], scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let value = self.emit(Instruction::LuauBufferNew { len });
            return Some(self.coerce_value(value, Type::Buffer, expected));
        }
        if member == "len" {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let buffer = match self.lower_expr(&args[0], scope, Some(Type::Buffer)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let value = self.emit(Instruction::LuauBufferLen { buffer });
            return Some(self.coerce_value(
                value,
                Type::Numeric(NumericType::I32),
                expected,
            ));
        }
        if member == "readbits" || member == "writebits" {
            let write = member == "writebits";
            let arity = if write { 4 } else { 3 };
            if args.len() != arity {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects {arity} arguments, got {}",
                    args.len()
                ))));
            }
            let buffer = match self.lower_expr(&args[0], scope, Some(Type::Buffer)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let f64_ty = Type::Numeric(NumericType::F64);
            let bit_offset = match self.lower_numeric_to(&args[1], &f64_ty, scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let bit_count = match self.lower_numeric_to(&args[2], &f64_ty, scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            if write {
                let i64_ty = Type::Numeric(NumericType::I64);
                let number = match self.lower_numeric_to(&args[3], &f64_ty, scope) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let wide = self.emit(Instruction::Cast {
                    value: number,
                    from: f64_ty,
                    to: i64_ty.clone(),
                });
                let stored = self.emit(Instruction::Cast {
                    value: wide,
                    from: i64_ty,
                    to: Type::Numeric(NumericType::I32),
                });
                let value = self.emit(Instruction::LuauBufferWriteBits {
                    buffer,
                    bit_offset,
                    bit_count,
                    value: stored,
                });
                return Some(self.coerce_value(value, Type::Unit, expected));
            }
            let value = self.emit(Instruction::LuauBufferReadBits {
                buffer,
                bit_offset,
                bit_count,
            });
            return Some(self.coerce_value(
                value,
                Type::Numeric(NumericType::U32),
                expected,
            ));
        }
        if member == "fromstring" {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let string = match self.lower_expr(&args[0], scope, Some(Type::String)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let value = self.emit(Instruction::LuauBufferFromString { string });
            return Some(self.coerce_value(value, Type::Buffer, expected));
        }
        if member == "tostring" {
            if args.len() != 1 {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 1 argument, got {}",
                    args.len()
                ))));
            }
            let buffer = match self.lower_expr(&args[0], scope, Some(Type::Buffer)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let value = self.emit(Instruction::LuauBufferToString { buffer });
            return Some(self.coerce_value(value, Type::String, expected));
        }
        if member == "readstring" {
            if args.len() != 3 {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 3 arguments, got {}",
                    args.len()
                ))));
            }
            let buffer = match self.lower_expr(&args[0], scope, Some(Type::Buffer)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let offset = match self.lower_luau_buffer_i32(&args[1], scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let count = match self.lower_luau_buffer_i32(&args[2], scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let value = self.emit(Instruction::LuauBufferReadString {
                buffer,
                offset,
                count,
            });
            return Some(self.coerce_value(value, Type::String, expected));
        }
        if member == "writestring" {
            if !matches!(args.len(), 3 | 4) {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 3 or 4 arguments, got {}",
                    args.len()
                ))));
            }
            let buffer = match self.lower_expr(&args[0], scope, Some(Type::Buffer)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let offset = match self.lower_luau_buffer_i32(&args[1], scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let string = match self.lower_expr(&args[2], scope, Some(Type::String)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let count = match args.get(3) {
                Some(expr) => match self.lower_luau_buffer_i32(expr, scope) {
                    Ok(value) => Some(value),
                    Err(error) => return Some(Err(error)),
                },
                None => None,
            };
            let value = self.emit(Instruction::LuauBufferWriteString {
                buffer,
                offset,
                string,
                count,
            });
            return Some(self.coerce_value(value, Type::Unit, expected));
        }
        if member == "copy" {
            if !(3..=5).contains(&args.len()) {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 3 to 5 arguments, got {}",
                    args.len()
                ))));
            }
            let target = match self.lower_expr(&args[0], scope, Some(Type::Buffer)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let target_offset = match self.lower_luau_buffer_i32(&args[1], scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let source = match self.lower_expr(&args[2], scope, Some(Type::Buffer)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let source_offset = match args.get(3) {
                Some(expr) => match self.lower_luau_buffer_i32(expr, scope) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                },
                None => self.emit_i32_const(0),
            };
            let count = match args.get(4) {
                Some(expr) => match self.lower_luau_buffer_i32(expr, scope) {
                    Ok(value) => Some(value),
                    Err(error) => return Some(Err(error)),
                },
                None => None,
            };
            let value = self.emit(Instruction::LuauBufferCopy {
                target,
                target_offset,
                source,
                source_offset,
                count,
            });
            return Some(self.coerce_value(value, Type::Unit, expected));
        }
        if member == "fill" {
            if !matches!(args.len(), 3 | 4) {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 3 or 4 arguments, got {}",
                    args.len()
                ))));
            }
            let buffer = match self.lower_expr(&args[0], scope, Some(Type::Buffer)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let offset = match self.lower_luau_buffer_i32(&args[1], scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let stored = match self.lower_luau_buffer_i32(&args[2], scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let count = match args.get(3) {
                Some(expr) => match self.lower_luau_buffer_i32(expr, scope) {
                    Ok(value) => Some(value),
                    Err(error) => return Some(Err(error)),
                },
                None => None,
            };
            let value = self.emit(Instruction::LuauBufferFill {
                buffer,
                offset,
                value: stored,
                count,
            });
            return Some(self.coerce_value(value, Type::Unit, expected));
        }
        let (write, kind) = if let Some(suffix) = member.strip_prefix("read") {
            (false, buffer_scalar_kind(suffix)?)
        } else {
            let suffix = member.strip_prefix("write")?;
            (true, buffer_scalar_kind(suffix)?)
        };
        let arity = if write { 3 } else { 2 };
        if args.len() != arity {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects {arity} arguments, got {}",
                args.len()
            ))));
        }
        let buffer = match self.lower_expr(&args[0], scope, Some(Type::Buffer)) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let offset = match self.lower_luau_buffer_i32(&args[1], scope) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        if write {
            let stored = match kind {
                TypedArrayKind::F32 | TypedArrayKind::F64 => {
                    let f64_ty = Type::Numeric(NumericType::F64);
                    let number = match self.lower_numeric_to(&args[2], &f64_ty, scope) {
                        Ok(value) => value,
                        Err(error) => return Some(Err(error)),
                    };
                    if kind == TypedArrayKind::F32 {
                        self.emit(Instruction::Cast {
                            value: number,
                            from: f64_ty,
                            to: Type::Numeric(NumericType::F32),
                        })
                    } else {
                        number
                    }
                }
                _ => {
                    let f64_ty = Type::Numeric(NumericType::F64);
                    let i64_ty = Type::Numeric(NumericType::I64);
                    let number = match self.lower_numeric_to(&args[2], &f64_ty, scope) {
                        Ok(value) => value,
                        Err(error) => return Some(Err(error)),
                    };
                    let wide = self.emit(Instruction::Cast {
                        value: number,
                        from: f64_ty,
                        to: i64_ty.clone(),
                    });
                    self.emit(Instruction::Cast {
                        value: wide,
                        from: i64_ty,
                        to: Type::Numeric(NumericType::I32),
                    })
                }
            };
            let value = self.emit(Instruction::LuauBufferSet {
                buffer,
                offset,
                value: stored,
                kind,
            });
            Some(self.coerce_value(value, Type::Unit, expected))
        } else {
            let value = self.emit(Instruction::LuauBufferGet {
                buffer,
                offset,
                kind,
            });
            Some(self.coerce_value(
                value,
                Type::Numeric(kind.element_numeric_type()),
                expected,
            ))
        }
    }

    fn infer_buffer_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let member = name.strip_prefix("buffer.")?;
        let (params, result) = match member {
            "create" => (vec![Type::number()], Type::Buffer),
            "len" => (vec![Type::Buffer], Type::Numeric(NumericType::I32)),
            "readbits" => (
                vec![Type::Buffer, Type::number(), Type::number()],
                Type::Numeric(NumericType::U32),
            ),
            "writebits" => (
                vec![Type::Buffer, Type::number(), Type::number(), Type::number()],
                Type::Unit,
            ),
            "fromstring" => (vec![Type::String], Type::Buffer),
            "tostring" => (vec![Type::Buffer], Type::String),
            "readstring" => (
                vec![Type::Buffer, Type::number(), Type::number()],
                Type::String,
            ),
            "writestring" if matches!(args.len(), 3 | 4) => {
                let mut params = vec![Type::Buffer, Type::number(), Type::String];
                if args.len() == 4 {
                    params.push(Type::number());
                }
                (params, Type::Unit)
            }
            "copy" if (3..=5).contains(&args.len()) => {
                let mut params = vec![Type::Buffer, Type::number(), Type::Buffer];
                if args.len() >= 4 {
                    params.push(Type::number());
                }
                if args.len() == 5 {
                    params.push(Type::number());
                }
                (params, Type::Unit)
            }
            "fill" if matches!(args.len(), 3 | 4) => {
                let mut params = vec![Type::Buffer, Type::number(), Type::number()];
                if args.len() == 4 {
                    params.push(Type::number());
                }
                (params, Type::Unit)
            }
            member if member.starts_with("read") => {
                let kind = buffer_scalar_kind(member.strip_prefix("read")?)?;
                (
                    vec![Type::Buffer, Type::number()],
                    Type::Numeric(kind.element_numeric_type()),
                )
            }
            member if member.starts_with("write") => {
                buffer_scalar_kind(member.strip_prefix("write")?)?;
                (
                    vec![Type::Buffer, Type::number(), Type::number()],
                    Type::Unit,
                )
            }
            _ => return None,
        };
        if args.len() != params.len() {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects {} arguments, got {}",
                params.len(),
                args.len()
            ))));
        }
        for (arg, target) in args.iter().zip(params) {
            if let Err(error) = self.infer_expr_type(arg, types, Some(target)) {
                return Some(Err(error));
            }
        }
        Some(coerce_type(result, expected))
    }

    fn infer_typed_array_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let kind = typed_array_create_kind(name)?;
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let length_ty = match self
            .infer_expr_type(&args[0], types, Some(Type::Numeric(NumericType::I32)))
            .or_else(|_| self.infer_expr_type(&args[0], types, None))
            .map(first_of_multi)
        {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if !length_ty.is_numeric() {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects a numeric length, got {length_ty}"
            ))));
        }
        Some(coerce_type(Type::TypedArray(kind), expected))
    }

    /// Append every element of `src` (an `Array(Unknown)` value) to the
    /// growable array `dest` via an inline counted loop, using the
    /// write-at-length append semantics of growable arrays.
    fn emit_array_append_all(&mut self, dest: ValueId, src: ValueId) {
        let i32_ty = Type::Numeric(NumericType::I32);
        let zero = self.emit_i32_const(0);
        let one = self.emit_i32_const(1);
        let src_len = self.emit(Instruction::ArrayLen { array: src });

        let preheader = self.current_block;
        let header = self.new_block();
        let body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        self.current_block = header;
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, zero)]));
        // Loop-invariant, threaded through a phi with a `+ 0` self-edge so the
        // local allocator treats it as live across the back edge (see
        // lower_table_insert's shift loop).
        let len_phi = self.emit(Instruction::Phi(vec![(preheader, src_len)]));
        let cond = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index_phi,
            right: len_phi,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: cond,
                then_block: body,
                else_block: exit,
            },
        );

        self.current_block = body;
        let element = self.emit(Instruction::ArrayGet {
            array: src,
            index: index_phi,
            element_ty: Type::Unknown,
        });
        let dest_len = self.emit(Instruction::ArrayLen { array: dest });
        self.emit(Instruction::ArraySet {
            array: dest,
            index: dest_len,
            value: element,
            element_ty: Type::Unknown,
        });
        let next_index = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: index_phi,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty.clone(),
        });
        let next_len = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: len_phi,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty,
        });
        let body_exit = self.current_block;
        self.set_terminator(body_exit, Terminator::Jump(header));
        add_phi_incoming(
            &mut self.function,
            header,
            index_phi,
            (body_exit, next_index),
        );
        add_phi_incoming(&mut self.function, header, len_phi, (body_exit, next_len));
        self.current_block = exit;
    }

    /// Infer the element type of the array passed as a table builtin's first argument.
    fn table_array_element_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Type, Diagnostic> {
        let arg = args
            .first()
            .ok_or_else(|| Diagnostic::new(format!("{name} expects an array argument")))?;
        let list_ty = self.infer_expr_type(arg, types, None)?;
        match list_ty {
            Type::Array(element) => Ok(Arc::unwrap_or_clone(element)),
            other => Err(Diagnostic::new(format!(
                "{name} expects an array as its first argument, got {other}"
            ))),
        }
    }

    fn lower_table_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        match name {
            TABLE_CONCAT => {}
            TABLE_PACK => return Some(self.lower_table_pack(args, scope, expected)),
            TABLE_CREATE => return Some(self.lower_table_create(args, scope, expected)),
            TABLE_UNPACK => return Some(self.lower_table_unpack(args, scope, expected)),
            TABLE_GETN => return Some(self.lower_table_getn(args, scope, expected)),
            TABLE_INSERT => return Some(self.lower_table_insert(args, scope, expected)),
            TABLE_REMOVE => return Some(self.lower_table_remove(args, scope, expected)),
            TABLE_SORT => return Some(self.lower_table_sort(args, scope, expected)),
            _ => return None,
        }
        if args.is_empty() || args.len() > 4 {
            return Some(Err(Diagnostic::new(format!(
                "{TABLE_CONCAT} expects 1 to 4 arguments, got {}",
                args.len()
            ))));
        }
        let array_ty = Type::Array(Arc::new(Type::String));
        let list_ty = match self
            .infer_expr_type(&args[0], &scope.types, Some(array_ty.clone()))
            .or_else(|_| self.infer_expr_type(&args[0], &scope.types, None))
        {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        let Some(element_ty) = table_concat_element_type(&list_ty) else {
            return Some(Err(Diagnostic::new(format!(
                "{TABLE_CONCAT} expects an array of strings or numbers, got {list_ty}"
            ))));
        };
        let array_value = match self.lower_expr(&args[0], scope, Some(list_ty.clone())) {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        };
        let empty_string = self.emit(Instruction::String(String::new()));
        let separator = if let Some(separator_expr) = args.get(1) {
            match self.infer_expr_type(separator_expr, &scope.types, None) {
                Ok(Type::String) => {}
                Ok(ty) => {
                    return Some(Err(Diagnostic::new(format!(
                        "{TABLE_CONCAT} expects a string separator, got {ty}"
                    ))));
                }
                Err(error) => return Some(Err(error)),
            }
            match self.lower_expr(separator_expr, scope, Some(Type::String)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            }
        } else {
            empty_string
        };

        let len = self.emit(Instruction::ArrayLen { array: array_value });
        let zero = self.emit(Instruction::Number {
            ty: NumericType::I32,
            literal: NumberLiteral { raw: "0".into() },
        });
        let one = self.emit(Instruction::Number {
            ty: NumericType::I32,
            literal: NumberLiteral { raw: "1".into() },
        });
        // Authored table.concat bounds are 1-based and inclusive. A physical
        // start of `i - 1` paired with an exclusive stop of authored `j`
        // expresses that range directly while preserving empty i > j ranges.
        let start = if let Some(start_expr) = args.get(2) {
            match self.lower_authored_array_index(start_expr, scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            }
        } else {
            zero
        };
        let stop = if let Some(stop_expr) = args.get(3) {
            match self.lower_index_to_i32(stop_expr, scope) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            }
        } else {
            len
        };

        // Naive lowering: result = "" then for each element, result = result .. prefix .. element,
        // where prefix starts as "" and becomes the separator after the first element. This avoids
        // a conditional inside the loop body while still placing the separator only between items.
        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        self.current_block = header;
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, start)]));
        let acc_phi = self.emit(Instruction::Phi(vec![(preheader, empty_string)]));
        let prefix_phi = self.emit(Instruction::Phi(vec![(preheader, empty_string)]));
        // `len` is loop-invariant, but it must still be threaded through a phi (with a
        // trivial `+ 0` self-edge) so the local allocator's liveness analysis treats it as
        // live across the back-edge — mirrors `array_len_phi` in `lower_for_in`.
        let len_phi = self.emit(Instruction::Phi(vec![(preheader, stop)]));
        let cond = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index_phi,
            right: len_phi,
            operand_ty: Type::Numeric(NumericType::I32),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: cond,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let raw_element = self.emit(Instruction::ArrayGet {
            array: array_value,
            index: index_phi,
            element_ty: element_ty.clone(),
        });
        // Numeric elements join as their `tostring` text, matching Luau.
        let element = self.tostring_value(raw_element, element_ty);
        let with_prefix = self.emit(Instruction::Binary {
            op: BinaryOp::Concat,
            left: acc_phi,
            right: prefix_phi,
            operand_ty: Type::String,
            result_ty: Type::String,
        });
        let next_acc = self.emit(Instruction::Binary {
            op: BinaryOp::Concat,
            left: with_prefix,
            right: element,
            operand_ty: Type::String,
            result_ty: Type::String,
        });
        let next_index = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: index_phi,
            right: one,
            operand_ty: Type::Numeric(NumericType::I32),
            result_ty: Type::Numeric(NumericType::I32),
        });
        let next_len = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: len_phi,
            right: zero,
            operand_ty: Type::Numeric(NumericType::I32),
            result_ty: Type::Numeric(NumericType::I32),
        });
        let body_exit = self.current_block;
        self.set_terminator(body_exit, Terminator::Jump(header));
        add_phi_incoming(&mut self.function, header, index_phi, (body_exit, next_index));
        add_phi_incoming(&mut self.function, header, acc_phi, (body_exit, next_acc));
        add_phi_incoming(&mut self.function, header, prefix_phi, (body_exit, separator));
        add_phi_incoming(&mut self.function, header, len_phi, (body_exit, next_len));

        self.current_block = exit;
        Some(self.coerce_value(acc_phi, Type::String, expected))
    }

    fn emit_i32_const(&mut self, value: i32) -> ValueId {
        self.emit(Instruction::Number {
            ty: NumericType::I32,
            literal: NumberLiteral {
                raw: value.to_string(),
            },
        })
    }

    fn lower_table_getn(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new(format!(
                "{TABLE_GETN} expects 1 argument, got {}",
                args.len()
            )));
        }
        let element_ty = self.table_array_element_type(TABLE_GETN, args, &scope.types)?;
        let array_ty = Type::Array(Arc::new(element_ty));
        let array = self.lower_expr(&args[0], scope, Some(array_ty))?;
        let len = self.emit(Instruction::ArrayLen { array });
        self.coerce_value(len, Type::Numeric(NumericType::I32), expected)
    }

    /// `table.pack(...)` collects its arguments into a fresh `{unknown}`
    /// array whose length doubles as the packed count (`t.n`, `#t`).
    fn lower_table_pack(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        if args.len() > 1 {
            for arg in &args[..args.len() - 1] {
                if matches!(arg, Expr::Vararg(_)) {
                    return Err(Diagnostic::new(
                        "'...' is only supported as the last argument of a call",
                    ));
                }
            }
        }
        let array_ty = Type::Array(Arc::new(Type::Unknown));
        let trailing_vararg = matches!(args.last(), Some(Expr::Vararg(_)));
        let explicit = if trailing_vararg {
            &args[..args.len() - 1]
        } else {
            args
        };
        let prefix = explicit
            .iter()
            .map(|arg| self.lower_expr(arg, scope, Some(Type::Unknown)))
            .collect::<Result<Vec<_>, _>>()?;
        if !trailing_vararg {
            let value = self.emit(Instruction::ArrayNew {
                element_ty: Type::Unknown,
                elements: prefix,
            });
            return self.coerce_value(value, array_ty, expected);
        }
        let varargs = self
            .vararg_value
            .ok_or_else(|| Diagnostic::new("'...' used outside a vararg function"))?;
        let value = if prefix.is_empty() {
            // `table.pack(...)` is a fresh copy of the caller's varargs.
            let zero = self.emit_i32_const(0);
            self.emit(Instruction::ArraySlice {
                array: varargs,
                start: zero,
                element_ty: Type::Unknown,
            })
        } else {
            let dest = self.emit(Instruction::ArrayNew {
                element_ty: Type::Unknown,
                elements: prefix,
            });
            self.emit_array_append_all(dest, varargs);
            dest
        };
        self.coerce_value(value, array_ty, expected)
    }

    /// `table.create(count, value)` builds a growable array of `count` copies
    /// of `value` via an inline counted append loop. The one-argument form is
    /// an empty array: the capacity hint has no effect on growable arrays.
    fn lower_table_create(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        if args.is_empty() || args.len() > 2 {
            return Err(Diagnostic::new(format!(
                "{TABLE_CREATE} expects 1 or 2 arguments, got {}",
                args.len()
            )));
        }
        let expected_element = match &expected {
            Some(Type::Array(element)) => Some((**element).clone()),
            _ => None,
        };
        let element_ty = match args.get(1) {
            Some(value) => first_of_multi(self.infer_expr_type(value, &scope.types, expected_element)?),
            None => expected_element.unwrap_or(Type::Unknown),
        };
        let count = self.lower_index_to_i32(&args[0], scope)?;
        let array = self.emit(Instruction::ArrayNew {
            element_ty: element_ty.clone(),
            elements: Vec::new(),
        });
        if let Some(value_expr) = args.get(1) {
            let value = self.lower_expr(value_expr, scope, Some(element_ty.clone()))?;
            let i32_ty = Type::Numeric(NumericType::I32);
            let zero = self.emit_i32_const(0);
            let one = self.emit_i32_const(1);

            let preheader = self.current_block;
            let header = self.new_block();
            let body = self.new_block();
            let exit = self.new_block();
            self.set_terminator(preheader, Terminator::Jump(header));

            self.current_block = header;
            let index_phi = self.emit(Instruction::Phi(vec![(preheader, zero)]));
            // `count` is loop-invariant, but it must still be threaded through
            // a phi (with a trivial `+ 0` self-edge) so the local allocator's
            // liveness analysis treats it as live across the back edge —
            // mirrors the table.concat loop above.
            let count_phi = self.emit(Instruction::Phi(vec![(preheader, count)]));
            let cond = self.emit(Instruction::Binary {
                op: BinaryOp::Less,
                left: index_phi,
                right: count_phi,
                operand_ty: i32_ty.clone(),
                result_ty: Type::Bool,
            });
            self.set_terminator(
                header,
                Terminator::Branch {
                    condition: cond,
                    then_block: body,
                    else_block: exit,
                },
            );

            self.current_block = body;
            // Write-at-length appends grow the array by one element.
            let len = self.emit(Instruction::ArrayLen { array });
            self.emit(Instruction::ArraySet {
                array,
                index: len,
                value,
                element_ty: element_ty.clone(),
            });
            let next_index = self.emit(Instruction::Binary {
                op: BinaryOp::Add,
                left: index_phi,
                right: one,
                operand_ty: i32_ty.clone(),
                result_ty: i32_ty.clone(),
            });
            let next_count = self.emit(Instruction::Binary {
                op: BinaryOp::Add,
                left: count_phi,
                right: zero,
                operand_ty: i32_ty.clone(),
                result_ty: i32_ty,
            });
            let body_exit = self.current_block;
            self.set_terminator(body_exit, Terminator::Jump(header));
            add_phi_incoming(
                &mut self.function,
                header,
                index_phi,
                (body_exit, next_index),
            );
            add_phi_incoming(
                &mut self.function,
                header,
                count_phi,
                (body_exit, next_count),
            );
            self.current_block = exit;
        }
        self.coerce_value(array, Type::Array(Arc::new(element_ty)), expected)
    }

    /// `table.unpack(a [, first [, last]])` with statically-knowable bounds
    /// lowers to a fixed tuple of element loads. The authored bounds are
    /// Luau-compatible and 1-based, defaulting to the whole array; the
    /// runtime-variable arity case is not supported yet.
    fn lower_table_unpack(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        if args.is_empty() || args.len() > 3 {
            return Err(Diagnostic::new(format!(
                "{TABLE_UNPACK} expects 1 to 3 arguments, got {}",
                args.len()
            )));
        }
        let element_ty = self.table_array_element_type(TABLE_UNPACK, args, &scope.types)?;
        let indices = table_unpack_static_indices(args, expected.as_ref())?;
        let array = self.lower_expr(
            &args[0],
            scope,
            Some(Type::Array(Arc::new(element_ty.clone()))),
        )?;
        // The bound arguments are compile-time literals (checked above), so
        // they need no lowering of their own.
        let mut values = Vec::with_capacity(indices.len());
        for index in indices {
            let index = self.emit_i32_const(index);
            values.push(self.emit(Instruction::ArrayGet {
                array,
                index,
                element_ty: element_ty.clone(),
            }));
        }
        let result_types = vec![element_ty; values.len()];
        let packed = self.emit(Instruction::PackMulti {
            values,
            types: result_types.clone(),
        });
        self.coerce_value(packed, Type::Multi(result_types), expected)
    }

    fn lower_table_insert(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        if args.len() < 2 || args.len() > 3 {
            return Err(Diagnostic::new(format!(
                "{TABLE_INSERT} expects 2 or 3 arguments, got {}",
                args.len()
            )));
        }
        let element_ty = self.table_array_element_type(TABLE_INSERT, args, &scope.types)?;
        // The value is lowered against the source element type, so a tagged-union
        // constructor still resolves against the variants the element declares;
        // the cells it is stored into are annotated with what they actually hold.
        let stored_ty = to_runtime_type(&element_ty);
        let array_ty = Type::Array(Arc::new(stored_ty.clone()));
        let array = self.lower_expr(&args[0], scope, Some(array_ty))?;
        let i32_ty = Type::Numeric(NumericType::I32);

        if args.len() == 2 {
            // table.insert(t, v): append at index #t.
            let value = self.lower_expr(&args[1], scope, Some(element_ty.clone()))?;
            let len = self.emit(Instruction::ArrayLen { array });
            self.emit(Instruction::ArraySet {
                array,
                index: len,
                value,
                element_ty: stored_ty,
            });
            let unit = self.emit(Instruction::Unit);
            return self.coerce_value(unit, Type::Unit, expected);
        }

        // table.insert(t, pos, v): authored positions are 1-based. Convert to
        // a physical offset, append v to grow by one, then shift the tail.
        let pos = self.lower_authored_array_index(&args[1], scope)?;
        let value = self.lower_expr(&args[2], scope, Some(element_ty.clone()))?;
        let element_ty = stored_ty;
        let n = self.emit(Instruction::ArrayLen { array });
        let zero = self.emit_i32_const(0);
        let one = self.emit_i32_const(1);

        // Validate before growing: authored positions 1..=#t + 1 map to
        // physical offsets 0..=#t. Without this guard, #t + 2 would become
        // the append position after the temporary growth below and append a
        // second element instead of failing.
        let non_negative = self.emit(Instruction::Binary {
            op: BinaryOp::GreaterEq,
            left: pos,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        let within_append_position = self.emit(Instruction::Binary {
            op: BinaryOp::LessEq,
            left: pos,
            right: n,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        let valid = self.emit(Instruction::Binary {
            op: BinaryOp::And,
            left: non_negative,
            right: within_append_position,
            operand_ty: Type::Bool,
            result_ty: Type::Bool,
        });
        let insert_block = self.new_block();
        let trap_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition: valid,
                then_block: insert_block,
                else_block: trap_block,
            },
        );
        self.current_block = trap_block;
        let error = self.emit(Instruction::String("array index out of bounds".to_string()));
        self.emit(Instruction::Throw { error });
        self.set_terminator(
            trap_block,
            Terminator::Unreachable {
                span: args[1].span(),
            },
        );
        self.current_block = insert_block;
        self.emit(Instruction::ArraySet {
            array,
            index: n,
            value,
            element_ty: element_ty.clone(),
        });

        // for i = n down to pos + 1: t[i] = t[i-1]
        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        self.current_block = header;
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, n)]));
        // Loop-invariant, threaded through a phi with a `+ 0` self-edge so the
        // local allocator treats it as live across the back edge (see
        // lower_table_builtin_call's concat loop).
        let pos_phi = self.emit(Instruction::Phi(vec![(preheader, pos)]));
        let cond = self.emit(Instruction::Binary {
            op: BinaryOp::Greater,
            left: index_phi,
            right: pos_phi,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: cond,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let prev_index = self.emit(Instruction::Binary {
            op: BinaryOp::Sub,
            left: index_phi,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty.clone(),
        });
        let shifted = self.emit(Instruction::ArrayGet {
            array,
            index: prev_index,
            element_ty: element_ty.clone(),
        });
        self.emit(Instruction::ArraySet {
            array,
            index: index_phi,
            value: shifted,
            element_ty: element_ty.clone(),
        });
        let next_pos = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: pos_phi,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty,
        });
        let body_exit = self.current_block;
        self.set_terminator(body_exit, Terminator::Jump(header));
        add_phi_incoming(
            &mut self.function,
            header,
            index_phi,
            (body_exit, prev_index),
        );
        add_phi_incoming(&mut self.function, header, pos_phi, (body_exit, next_pos));

        self.current_block = exit;
        self.emit(Instruction::ArraySet {
            array,
            index: pos_phi,
            value,
            element_ty,
        });
        let unit = self.emit(Instruction::Unit);
        self.coerce_value(unit, Type::Unit, expected)
    }

    fn lower_table_remove(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        if args.is_empty() || args.len() > 2 {
            return Err(Diagnostic::new(format!(
                "{TABLE_REMOVE} expects 1 or 2 arguments, got {}",
                args.len()
            )));
        }
        let element_ty = self.table_array_element_type(TABLE_REMOVE, args, &scope.types)?;
        let array_ty = Type::Array(Arc::new(element_ty.clone()));
        let array = self.lower_expr(&args[0], scope, Some(array_ty))?;
        let i32_ty = Type::Numeric(NumericType::I32);

        let n = self.emit(Instruction::ArrayLen { array });
        let zero = self.emit_i32_const(0);
        let one = self.emit_i32_const(1);
        let pos = if let Some(pos_arg) = args.get(1) {
            self.lower_authored_array_index(pos_arg, scope)?
        } else {
            self.emit(Instruction::Binary {
                op: BinaryOp::Sub,
                left: n,
                right: one,
                operand_ty: i32_ty.clone(),
                result_ty: i32_ty.clone(),
            })
        };
        // Read the removed element up front (this also traps on out-of-range
        // positions, including removal from an empty array).
        let removed = self.emit(Instruction::ArrayGet {
            array,
            index: pos,
            element_ty: element_ty.clone(),
        });
        let limit = self.emit(Instruction::Binary {
            op: BinaryOp::Sub,
            left: n,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty.clone(),
        });

        // for i = pos while i < n - 1: t[i] = t[i+1]
        let preheader = self.current_block;
        let header = self.new_block();
        let loop_body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        self.current_block = header;
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, pos)]));
        // Loop-invariant `n - 1`, threaded with a `+ 0` self-edge (see the
        // concat loop for why).
        let limit_phi = self.emit(Instruction::Phi(vec![(preheader, limit)]));
        let cond = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index_phi,
            right: limit_phi,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: cond,
                then_block: loop_body,
                else_block: exit,
            },
        );

        self.current_block = loop_body;
        let next_index = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: index_phi,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty.clone(),
        });
        let shifted = self.emit(Instruction::ArrayGet {
            array,
            index: next_index,
            element_ty: element_ty.clone(),
        });
        self.emit(Instruction::ArraySet {
            array,
            index: index_phi,
            value: shifted,
            element_ty: element_ty.clone(),
        });
        let next_limit = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: limit_phi,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty,
        });
        let body_exit = self.current_block;
        self.set_terminator(body_exit, Terminator::Jump(header));
        add_phi_incoming(
            &mut self.function,
            header,
            index_phi,
            (body_exit, next_index),
        );
        add_phi_incoming(&mut self.function, header, limit_phi, (body_exit, next_limit));

        self.current_block = exit;
        self.emit(Instruction::ArrayPop {
            array,
            element_ty: element_ty.clone(),
        });
        self.coerce_value(removed, element_ty, expected)
    }

    fn lower_table_sort(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        if args.is_empty() {
            return Err(Diagnostic::new(format!(
                "{TABLE_SORT} expects at least 1 argument, got {}",
                args.len()
            )));
        }
        let element_ty = self.table_array_element_type(TABLE_SORT, args, &scope.types)?;
        let array_ty = Type::Array(Arc::new(element_ty.clone()));
        let array = self.lower_expr(&args[0], scope, Some(array_ty))?;
        let i32_ty = Type::Numeric(NumericType::I32);

        let comparator = if let Some(comparator_arg) = args.get(1) {
            let comparator_ty = Type::Function {
                params: vec![element_ty.clone(), element_ty.clone()],
                return_type: Arc::new(Type::Bool),
                has_self: false,
            };
            Some(self.lower_expr(comparator_arg, scope, Some(comparator_ty))?)
        } else {
            if !element_ty.is_numeric() && element_ty != Type::String {
                return Err(Diagnostic::new(format!(
                    "{TABLE_SORT} without a comparator requires numeric or string elements, got {element_ty}"
                )));
            }
            None
        };
        for arg in args.iter().skip(2) {
            let _ = self.lower_expr(arg, scope, None)?;
        }

        let n = self.emit(Instruction::ArrayLen { array });
        let zero = self.emit_i32_const(0);
        let one = self.emit_i32_const(1);

        // Insertion sort:
        //   for i = 1 while i < n:
        //     key = t[i]; j = i
        //     while j > 0 and lt(key, t[j-1]): t[j] = t[j-1]; j -= 1
        //     t[j] = key; i += 1
        let outer_pre = self.current_block;
        let outer_header = self.new_block();
        let outer_body = self.new_block();
        let inner_header = self.new_block();
        let inner_check = self.new_block();
        let inner_body = self.new_block();
        let inner_exit = self.new_block();
        let done = self.new_block();
        self.set_terminator(outer_pre, Terminator::Jump(outer_header));

        self.current_block = outer_header;
        let i_phi = self.emit(Instruction::Phi(vec![(outer_pre, one)]));
        // Loop-invariant length, threaded with a `+ 0` self-edge (see the
        // concat loop for why).
        let n_phi = self.emit(Instruction::Phi(vec![(outer_pre, n)]));
        let outer_cond = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: i_phi,
            right: n_phi,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            outer_header,
            Terminator::Branch {
                condition: outer_cond,
                then_block: outer_body,
                else_block: done,
            },
        );

        self.current_block = outer_body;
        let key = self.emit(Instruction::ArrayGet {
            array,
            index: i_phi,
            element_ty: element_ty.clone(),
        });
        self.set_terminator(outer_body, Terminator::Jump(inner_header));

        self.current_block = inner_header;
        let j_phi = self.emit(Instruction::Phi(vec![(outer_body, i_phi)]));
        let inner_cond = self.emit(Instruction::Binary {
            op: BinaryOp::Greater,
            left: j_phi,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            inner_header,
            Terminator::Branch {
                condition: inner_cond,
                then_block: inner_check,
                else_block: inner_exit,
            },
        );

        self.current_block = inner_check;
        let prev_index = self.emit(Instruction::Binary {
            op: BinaryOp::Sub,
            left: j_phi,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty.clone(),
        });
        let prev = self.emit(Instruction::ArrayGet {
            array,
            index: prev_index,
            element_ty: element_ty.clone(),
        });
        let should_shift = if let Some(comparator) = comparator {
            self.emit(Instruction::CallValue {
                callee: comparator,
                args: vec![key, prev],
                params: vec![element_ty.clone(), element_ty.clone()],
                return_type: Type::Bool,
            })
        } else {
            self.emit(Instruction::Binary {
                op: BinaryOp::Less,
                left: key,
                right: prev,
                operand_ty: element_ty.clone(),
                result_ty: Type::Bool,
            })
        };
        self.set_terminator(
            inner_check,
            Terminator::Branch {
                condition: should_shift,
                then_block: inner_body,
                else_block: inner_exit,
            },
        );

        self.current_block = inner_body;
        self.emit(Instruction::ArraySet {
            array,
            index: j_phi,
            value: prev,
            element_ty: element_ty.clone(),
        });
        let inner_body_exit = self.current_block;
        self.set_terminator(inner_body_exit, Terminator::Jump(inner_header));
        add_phi_incoming(
            &mut self.function,
            inner_header,
            j_phi,
            (inner_body_exit, prev_index),
        );

        self.current_block = inner_exit;
        self.emit(Instruction::ArraySet {
            array,
            index: j_phi,
            value: key,
            element_ty,
        });
        let next_i = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: i_phi,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty.clone(),
        });
        let next_n = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: n_phi,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty,
        });
        let inner_exit_block = self.current_block;
        self.set_terminator(inner_exit_block, Terminator::Jump(outer_header));
        add_phi_incoming(
            &mut self.function,
            outer_header,
            i_phi,
            (inner_exit_block, next_i),
        );
        add_phi_incoming(
            &mut self.function,
            outer_header,
            n_phi,
            (inner_exit_block, next_n),
        );

        self.current_block = done;
        let unit = self.emit(Instruction::Unit);
        self.coerce_value(unit, Type::Unit, expected)
    }

    fn lower_print_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        if name != PRINT {
            return None;
        }
        // Luau semantics: stringify every argument and join the pieces with
        // tabs into the single string the host's `print` import receives. A
        // multi-value result expands only in the last argument position;
        // anywhere else Lua's adjustment rule keeps its first value.
        let mut pieces: Vec<ValueId> = Vec::new();
        for (arg_index, arg) in args.iter().enumerate() {
            let is_last = arg_index + 1 == args.len();
            let arg_ty = match self.infer_expr_type(arg, &scope.types, None) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            if let Type::Multi(parts) = arg_ty {
                let tuple = match self.lower_expr(arg, scope, None) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let printed = if is_last { parts.len() } else { 1.min(parts.len()) };
                for (index, part) in parts.into_iter().take(printed).enumerate() {
                    if !tostring_supported_type(&part) {
                        return Some(Err(Diagnostic::new(format!(
                            "{PRINT} cannot convert a {part} value",
                        ))));
                    }
                    if part == Type::Nil {
                        pieces.push(self.emit(Instruction::String("nil".to_string())));
                        continue;
                    }
                    let value = self.emit(Instruction::MultiGet {
                        value: tuple,
                        index,
                        ty: part.clone(),
                    });
                    pieces.push(self.tostring_value(value, part));
                }
            } else {
                match self.lower_tostring_operand(PRINT, arg, scope) {
                    Ok(value) => pieces.push(value),
                    Err(error) => return Some(Err(error)),
                }
            }
        }
        let mut pieces = pieces.into_iter();
        let message = match pieces.next() {
            None => self.emit(Instruction::String(String::new())),
            Some(first) => pieces.fold(first, |message, piece| {
                let tab = self.emit(Instruction::String("\t".to_string()));
                let with_tab = self.emit(Instruction::Binary {
                    op: BinaryOp::Concat,
                    left: message,
                    right: tab,
                    operand_ty: Type::String,
                    result_ty: Type::String,
                });
                self.emit(Instruction::Binary {
                    op: BinaryOp::Concat,
                    left: with_tab,
                    right: piece,
                    operand_ty: Type::String,
                    result_ty: Type::String,
                })
            }),
        };
        let print_value = self.emit(Instruction::Print { value: message });
        Some(self.coerce_value(print_value, Type::Unit, expected))
    }

    fn infer_bit32_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let (arity, result_ty) = match name {
            BIT32_BNOT | BIT32_BYTESWAP | BIT32_COUNTLZ | BIT32_COUNTRZ => {
                (Some(1), Type::Numeric(NumericType::U32))
            }
            BIT32_LROTATE | BIT32_RROTATE | BIT32_LSHIFT | BIT32_RSHIFT | BIT32_ARSHIFT => {
                (Some(2), Type::Numeric(NumericType::U32))
            }
            BIT32_EXTRACT if (2..=3).contains(&args.len()) => {
                (None, Type::Numeric(NumericType::U32))
            }
            BIT32_REPLACE if (3..=4).contains(&args.len()) => {
                (None, Type::Numeric(NumericType::U32))
            }
            BIT32_EXTRACT => {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 2 or 3 arguments, got {}",
                    args.len()
                ))));
            }
            BIT32_REPLACE => {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects 3 or 4 arguments, got {}",
                    args.len()
                ))));
            }
            BIT32_BAND | BIT32_BOR | BIT32_BXOR => (None, Type::Numeric(NumericType::U32)),
            BIT32_BTEST => (None, Type::Bool),
            _ => return None,
        };
        if let Some(arity) = arity {
            if args.len() != arity {
                return Some(Err(Diagnostic::new(format!(
                    "{name} expects {arity} argument{}, got {}",
                    if arity == 1 { "" } else { "s" },
                    args.len()
                ))));
            }
        }

        let u32_ty = Type::Numeric(NumericType::U32);
        let i32_ty = Type::Numeric(NumericType::I32);
        for (index, arg) in args.iter().enumerate() {
            let signed_position = matches!(
                name,
                BIT32_LROTATE | BIT32_RROTATE | BIT32_LSHIFT | BIT32_RSHIFT | BIT32_ARSHIFT
            ) && index == 1
                || name == BIT32_EXTRACT && index >= 1
                || name == BIT32_REPLACE && index >= 2;
            let expected_arg = if signed_position { i32_ty.clone() } else { u32_ty.clone() };
            let typed = self.infer_expr_type(arg, types, Some(expected_arg.clone()));
            if matches!(&typed, Ok(ty) if ty == &expected_arg) {
                continue;
            }
            if !self
                .infer_expr_type(arg, types, None)
                .map(first_of_multi)
                .as_ref()
                .is_ok_and(bit32_dynamic_operand)
            {
                return Some(Err(bit32_operand_mismatch(
                    name,
                    index,
                    &expected_arg,
                    typed,
                )));
            }
        }
        Some(Ok(result_ty))
    }

    fn infer_tostring_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != TO_STRING {
            return None;
        }
        let Expr::Call { args, .. } = call else {
            return None;
        };
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{TO_STRING} expects 1 argument, got {}",
                args.len()
            ))));
        }
        let arg_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if tostring_supported_type(&arg_ty) {
            Some(Ok(Type::String))
        } else {
            Some(Err(Diagnostic::new(format!(
                "{TO_STRING} cannot convert a {arg_ty} value",
            ))))
        }
    }

    fn infer_type_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if !matches!(name, TYPE | TYPEOF) {
            return None;
        }
        if args.len() != 1 {
            return Some(Err(Diagnostic::new(format!(
                "{name} expects 1 argument, got {}",
                args.len()
            ))));
        }
        match self.infer_expr_type(&args[0], types, None) {
            Ok(_) => Some(coerce_type(Type::String, expected)),
            Err(error) => Some(Err(error)),
        }
    }

    fn infer_tonumber_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != TO_NUMBER {
            return None;
        }
        if args.is_empty() || args.len() > 2 {
            return Some(Err(Diagnostic::new(format!(
                "{TO_NUMBER} expects 1 or 2 arguments, got {}",
                args.len()
            ))));
        }
        let arg_ty = match self.infer_expr_type(&args[0], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if !(arg_ty.is_numeric() || arg_ty == Type::String || arg_ty == Type::Unknown) {
            return Some(Err(Diagnostic::new(format!(
                "{TO_NUMBER} expects a numeric, string, or unknown argument, got {arg_ty}",
            ))));
        }
        if let Some(base) = args.get(1) {
            match self.infer_expr_type(base, types, Some(Type::Numeric(NumericType::I32))) {
                Ok(Type::Numeric(NumericType::I32)) => {}
                Ok(ty) => {
                    return Some(Err(Diagnostic::new(format!(
                        "{TO_NUMBER} expects base to be i32, got {ty}"
                    ))));
                }
                Err(error) => return Some(Err(error)),
            }
            if !(arg_ty == Type::String || arg_ty == Type::Unknown) {
                return Some(Err(Diagnostic::new(format!(
                    "{TO_NUMBER} with a base expects a string or unknown value, got {arg_ty}",
                ))));
            }
        }
        let f64_ty = Type::Numeric(NumericType::F64);
        if args.len() == 1 && arg_ty.is_numeric() {
            return Some(coerce_type(f64_ty, expected));
        }
        // Runtime parses yield `f64?` (nil on failure) unless the context
        // requires a plain number.
        if string_byte_requires_numeric_value(expected.as_ref()) {
            Some(coerce_type(f64_ty, expected))
        } else {
            Some(coerce_type(Type::Nullable(Arc::new(f64_ty)), expected))
        }
    }

    fn infer_select_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != SELECT {
            return None;
        }
        if args.len() != 2 {
            return Some(Err(Diagnostic::new(format!(
                "{SELECT} expects 2 arguments, got {}",
                args.len()
            ))));
        }
        let count_marker = matches!(&args[0], Expr::String(marker, _) if marker == "#");
        let arg_ty = match self.infer_expr_type(&args[1], types, None) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };

        if count_marker && arg_ty == Type::Unit {
            return Some(Ok(Type::Numeric(NumericType::I32)));
        }

        let Some(element_ty) = arg_ty.element_type() else {
            return Some(Err(Diagnostic::new(format!(
                "{SELECT} expects an array, got {arg_ty}"
            ))));
        };
        if count_marker {
            Some(Ok(Type::Numeric(NumericType::I32)))
        } else {
            Some(Ok(element_ty))
        }
    }

    fn infer_table_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if !matches!(
            name,
            TABLE_CONCAT
                | TABLE_INSERT
                | TABLE_REMOVE
                | TABLE_SORT
                | TABLE_GETN
                | TABLE_PACK
                | TABLE_CREATE
                | TABLE_UNPACK
        ) {
            return None;
        }
        let Expr::Call { args, .. } = call else {
            return None;
        };
        match name {
            TABLE_PACK => return Some(Ok(Type::Array(Arc::new(Type::Unknown)))),
            TABLE_CREATE => {
                let expected_element = match &expected {
                    Some(Type::Array(element)) => Some((**element).clone()),
                    _ => None,
                };
                let element_ty = match args.get(1) {
                    Some(value) => match self.infer_expr_type(value, types, expected_element) {
                        Ok(ty) => first_of_multi(ty),
                        Err(error) => return Some(Err(error)),
                    },
                    None => expected_element.unwrap_or(Type::Unknown),
                };
                return Some(Ok(Type::Array(Arc::new(element_ty))));
            }
            TABLE_UNPACK => {
                let element_ty = match self.table_array_element_type(TABLE_UNPACK, args, types) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                let indices = match table_unpack_static_indices(args, expected.as_ref()) {
                    Ok(indices) => indices,
                    Err(error) => return Some(Err(error)),
                };
                return Some(Ok(Type::Multi(vec![element_ty; indices.len()])));
            }
            TABLE_GETN => return Some(Ok(Type::Numeric(NumericType::I32))),
            TABLE_INSERT | TABLE_SORT => return Some(Ok(Type::Unit)),
            TABLE_REMOVE => {
                return Some(
                    self.table_array_element_type(TABLE_REMOVE, args, types),
                );
            }
            _ => {}
        }
        if args.is_empty() || args.len() > 4 {
            return Some(Err(Diagnostic::new(format!(
                "{TABLE_CONCAT} expects 1 to 4 arguments, got {}",
                args.len()
            ))));
        }
        let list_ty = match self
            .infer_expr_type(&args[0], types, Some(Type::Array(Arc::new(Type::String))))
            .or_else(|_| self.infer_expr_type(&args[0], types, None))
        {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        if table_concat_element_type(&list_ty).is_none() {
            return Some(Err(Diagnostic::new(format!(
                "{TABLE_CONCAT} expects an array of strings or numbers, got {list_ty}"
            ))));
        }
        if let Some(separator) = args.get(1) {
            match self.infer_expr_type(separator, types, None) {
                Ok(Type::String) => {}
                Ok(ty) => {
                    return Some(Err(Diagnostic::new(format!(
                        "{TABLE_CONCAT} expects a string separator, got {ty}"
                    ))));
                }
                Err(error) => return Some(Err(error)),
            }
        }
        for bound in args.iter().skip(2) {
            if let Err(error) = self.infer_expr_type(
                bound,
                types,
                Some(Type::Numeric(NumericType::I32)),
            ) {
                return Some(Err(error));
            }
        }
        Some(Ok(Type::String))
    }

    fn infer_print_builtin_call_type(
        &self,
        name: &str,
        call: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        if name != PRINT {
            return None;
        }
        let Expr::Call { args, .. } = call else {
            return None;
        };
        for (arg_index, arg) in args.iter().enumerate() {
            let is_last = arg_index + 1 == args.len();
            let arg_ty = match self.infer_expr_type(arg, types, None) {
                Ok(ty) => ty,
                Err(error) => return Some(Err(error)),
            };
            let parts = match arg_ty {
                // A multi-value result expands only in the last argument
                // position; anywhere else it adjusts to its first value.
                Type::Multi(parts) if is_last => parts,
                Type::Multi(parts) => parts.into_iter().take(1).collect(),
                ty => vec![ty],
            };
            for part in parts {
                if !tostring_supported_type(&part) {
                    return Some(Err(Diagnostic::new(format!(
                        "{PRINT} cannot convert a {part} value",
                    ))));
                }
            }
        }
        Some(Ok(Type::Unit))
    }

    fn infer_json_builtin_call_type(
        &self,
        name: &str,
        type_args: &[Type],
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        match name {
            JSON_PACK => {
                if !type_args.is_empty() {
                    return Some(Err(Diagnostic::new(
                        "json.pack infers its value type and does not accept type arguments",
                    )));
                }
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "json.pack expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let value_ty = match self.infer_expr_type(&args[0], types, None) {
                    Ok(ty) => ty,
                    Err(error) => return Some(Err(error)),
                };
                if !json_supported_type(&value_ty) {
                    return Some(Err(Diagnostic::new(format!(
                        "json.pack does not support values of type {value_ty}"
                    ))));
                }
                Some(coerce_type(Type::String, expected))
            }
            JSON_UNPACK => {
                if type_args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "json.unpack expects exactly 1 type argument, got {}",
                        type_args.len()
                    ))));
                }
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "json.unpack expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                match self.infer_expr_type(&args[0], types, Some(Type::String)) {
                    Ok(Type::String) => {}
                    Ok(actual) => {
                        return Some(Err(Diagnostic::new(format!(
                            "json.unpack expects a string argument, got {actual}"
                        ))));
                    }
                    Err(error) => return Some(Err(error)),
                }
                let target = &type_args[0];
                if !json_supported_type(target) {
                    return Some(Err(Diagnostic::new(format!(
                        "json.unpack does not support values of type {target}"
                    ))));
                }
                Some(coerce_type(
                    Type::Multi(vec![json_unpack_value_type(target), Type::String]),
                    expected,
                ))
            }
            _ => None,
        }
    }

    fn lower_json_builtin_call(
        &mut self,
        name: &str,
        type_args: &[Type],
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        match name {
            JSON_PACK => Some((|| {
                if !type_args.is_empty() {
                    return Err(Diagnostic::new(
                        "json.pack infers its value type and does not accept type arguments",
                    ));
                }
                if args.len() != 1 {
                    return Err(Diagnostic::new(format!(
                        "json.pack expects 1 argument, got {}",
                        args.len()
                    )));
                }
                let value_ty = self.infer_expr_type(&args[0], &scope.types, None)?;
                if !json_supported_type(&value_ty) {
                    return Err(Diagnostic::new(format!(
                        "json.pack does not support values of type {value_ty}"
                    )));
                }
                let value = self.lower_expr(&args[0], scope, Some(value_ty.clone()))?;
                self.emit_json_host_call(JSON_PACK_RESET_HOST, Vec::new(), Type::Unit)?;
                let root = self.lower_json_pack_value(value, &value_ty)?;
                let packed = self.emit_json_host_call(
                    JSON_PACK_FINISH_HOST,
                    vec![root],
                    Type::String,
                )?;
                self.coerce_value(packed, Type::String, expected)
            })()),
            JSON_UNPACK => Some((|| {
                if type_args.len() != 1 {
                    return Err(Diagnostic::new(format!(
                        "json.unpack expects exactly 1 type argument, got {}",
                        type_args.len()
                    )));
                }
                if args.len() != 1 {
                    return Err(Diagnostic::new(format!(
                        "json.unpack expects 1 argument, got {}",
                        args.len()
                    )));
                }
                let target = type_args[0].clone();
                if !json_supported_type(&target) {
                    return Err(Diagnostic::new(format!(
                        "json.unpack does not support values of type {target}"
                    )));
                }
                let text = self.lower_expr(&args[0], scope, Some(Type::String))?;
                let schema = self.emit(Instruction::String(json_schema(&target)?));
                let error = self.emit_json_host_call(
                    JSON_UNPACK_START_HOST,
                    vec![text, schema],
                    Type::String,
                )?;
                let empty = self.emit(Instruction::String(String::new()));
                let succeeded = self.emit(Instruction::Binary {
                    op: BinaryOp::Eq,
                    left: error,
                    right: empty,
                    operand_ty: Type::String,
                    result_ty: Type::Bool,
                });
                let success_block = self.new_block();
                let failure_block = self.new_block();
                let merge_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::Branch {
                        condition: succeeded,
                        then_block: success_block,
                        else_block: failure_block,
                    },
                );

                let unpacked_ty = json_unpack_value_type(&target);
                let unpacked_runtime_ty = to_runtime_type(&unpacked_ty);
                let multi_ty = Type::Multi(vec![unpacked_runtime_ty.clone(), Type::String]);

                self.current_block = success_block;
                let root = self.emit_i32_const(0);
                let decoded = self.lower_json_unpack_value(root, &target)?;
                let decoded = if unpacked_ty == target {
                    decoded
                } else {
                    self.emit(Instruction::Cast {
                        value: decoded,
                        from: to_runtime_type(&target),
                        to: unpacked_runtime_ty.clone(),
                    })
                };
                let success_error = self.emit(Instruction::String(String::new()));
                let success_exit = self.current_block;
                self.set_terminator(success_exit, Terminator::Jump(merge_block));

                self.current_block = failure_block;
                let nil = self.emit(Instruction::Null {
                    ty: unpacked_runtime_ty.clone(),
                });
                self.set_terminator(failure_block, Terminator::Jump(merge_block));

                self.current_block = merge_block;
                let value = self.emit(Instruction::Phi(vec![
                    (success_exit, decoded),
                    (failure_block, nil),
                ]));
                let error = self.emit(Instruction::Phi(vec![
                    (success_exit, success_error),
                    (failure_block, error),
                ]));
                let result = self.emit(Instruction::PackMulti {
                    values: vec![value, error],
                    types: vec![unpacked_runtime_ty, Type::String],
                });
                self.coerce_value(
                    result,
                    multi_ty,
                    expected.map(|ty| to_runtime_type(&ty)),
                )
            })()),
            _ => None,
        }
    }

    fn emit_json_host_call(
        &mut self,
        host_name: &str,
        args: Vec<ValueId>,
        return_type: Type,
    ) -> Result<ValueId, Diagnostic> {
        self.emit_string_host_call(host_name, args, return_type)
    }

    fn lower_json_pack_value(
        &mut self,
        value: ValueId,
        ty: &Type,
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        match ty {
            Type::Opaque { ty, .. } => {
                self.lower_json_pack_value(value, ty)
            }
            Type::Numeric(numeric) => self.emit_json_host_call(
                json_pack_numeric_host(*numeric),
                vec![value],
                i32_ty,
            ),
            Type::Bool => {
                self.emit_json_host_call(JSON_PACK_BOOL_HOST, vec![value], i32_ty)
            }
            Type::String | Type::StringLiteralUnion(_) => {
                self.emit_json_host_call(JSON_PACK_STRING_HOST, vec![value], i32_ty)
            }
            Type::Bytes => {
                self.emit_json_host_call(JSON_PACK_BYTES_HOST, vec![value], i32_ty)
            }
            Type::Unit => self.emit_json_host_call(JSON_PACK_NULL_HOST, Vec::new(), i32_ty),
            Type::Nullable(inner) => {
                let is_null = self.emit(Instruction::IsNull {
                    value,
                    ty: (**inner).clone(),
                });
                let null_block = self.new_block();
                let value_block = self.new_block();
                let merge_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::Branch {
                        condition: is_null,
                        then_block: null_block,
                        else_block: value_block,
                    },
                );
                self.current_block = null_block;
                let null_node =
                    self.emit_json_host_call(JSON_PACK_NULL_HOST, Vec::new(), i32_ty.clone())?;
                self.set_terminator(null_block, Terminator::Jump(merge_block));
                self.current_block = value_block;
                let inner_value = self.emit(Instruction::Cast {
                    value,
                    from: ty.clone(),
                    to: (**inner).clone(),
                });
                let value_node = self.lower_json_pack_value(inner_value, inner)?;
                let value_exit = self.current_block;
                self.set_terminator(value_exit, Terminator::Jump(merge_block));
                self.current_block = merge_block;
                Ok(self.emit(Instruction::Phi(vec![
                    (null_block, null_node),
                    (value_exit, value_node),
                ])))
            }
            Type::Array(element) => {
                let array_node = self.emit_json_host_call(
                    JSON_PACK_ARRAY_HOST,
                    Vec::new(),
                    i32_ty.clone(),
                )?;
                let len = self.emit(Instruction::ArrayLen { array: value });
                self.lower_json_pack_sequence_loop(
                    array_node,
                    len,
                    element,
                    |builder, index| {
                        Ok(builder.emit(Instruction::ArrayGet {
                            array: value,
                            index,
                            element_ty: to_runtime_type(element),
                        }))
                    },
                )?;
                Ok(array_node)
            }
            Type::TypedArray(kind) => {
                let array_node = self.emit_json_host_call(
                    JSON_PACK_ARRAY_HOST,
                    Vec::new(),
                    i32_ty.clone(),
                )?;
                let len = self.emit(Instruction::BufferLen { buffer: value });
                let element_ty = Type::Numeric(kind.element_numeric_type());
                self.lower_json_pack_sequence_loop(
                    array_node,
                    len,
                    &element_ty,
                    |builder, index| {
                        Ok(builder.emit(Instruction::BufferGet {
                            buffer: value,
                            index,
                            kind: *kind,
                        }))
                    },
                )?;
                Ok(array_node)
            }
            Type::Record(fields) => {
                let object = self.emit_json_host_call(
                    JSON_PACK_OBJECT_HOST,
                    Vec::new(),
                    i32_ty.clone(),
                )?;
                for (name, field_ty) in fields.iter() {
                    let field = self.emit(Instruction::StructGet {
                        base: value,
                        field: name.clone(),
                        field_ty: to_runtime_type(field_ty),
                    });
                    let node = self.lower_json_pack_value(field, field_ty)?;
                    let key = self.emit(Instruction::String(name.clone()));
                    self.emit_json_host_call(
                        JSON_PACK_OBJECT_SET_HOST,
                        vec![object, key, node],
                        Type::Unit,
                    )?;
                }
                Ok(object)
            }
            Type::TaggedVariant(variant) => {
                self.lower_json_pack_tagged(value, std::slice::from_ref(variant))
            }
            Type::TaggedUnion(variants) => self.lower_json_pack_tagged(value, variants),
            other => Err(Diagnostic::new(format!(
                "json.pack does not support values of type {other}"
            ))),
        }
    }

    fn lower_json_pack_sequence_loop(
        &mut self,
        array_node: ValueId,
        len: ValueId,
        element_ty: &Type,
        mut get_element: impl FnMut(&mut Self, ValueId) -> Result<ValueId, Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        let zero = self.emit_i32_const(0);
        let one = self.emit_i32_const(1);
        let entry = self.current_block;
        let header = self.new_block();
        let body = self.new_block();
        let done = self.new_block();
        self.set_terminator(entry, Terminator::Jump(header));
        self.current_block = header;
        let index = self.emit(Instruction::Phi(vec![(entry, zero)]));
        let in_bounds = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index,
            right: len,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: in_bounds,
                then_block: body,
                else_block: done,
            },
        );
        self.current_block = body;
        let element = get_element(self, index)?;
        let node = self.lower_json_pack_value(element, element_ty)?;
        self.emit_json_host_call(
            JSON_PACK_ARRAY_PUSH_HOST,
            vec![array_node, node],
            Type::Unit,
        )?;
        let next = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: index,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty,
        });
        let body_exit = self.current_block;
        self.set_terminator(body_exit, Terminator::Jump(header));
        add_phi_incoming(&mut self.function, header, index, (body_exit, next));
        self.current_block = done;
        Ok(())
    }

    fn lower_json_pack_tagged(
        &mut self,
        value: ValueId,
        variants: &[TaggedVariant],
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        let object = self.emit_json_host_call(
            JSON_PACK_OBJECT_HOST,
            Vec::new(),
            i32_ty.clone(),
        )?;
        let tag = self.emit(Instruction::StructGet {
            base: value,
            field: "tag".to_string(),
            field_ty: i32_ty.clone(),
        });
        let payload = self.emit(Instruction::StructGet {
            base: value,
            field: "value".to_string(),
            field_ty: Type::Unknown,
        });
        let merge = self.new_block();
        for (index, variant) in variants.iter().enumerate() {
            let variant_block = self.new_block();
            let next_dispatch = if index + 1 == variants.len() {
                self.set_terminator(self.current_block, Terminator::Jump(variant_block));
                None
            } else {
                let tag_id = self.emit_i32_const(self.variant_tag_id(&variant.tag)?);
                let matches = self.emit(Instruction::Binary {
                    op: BinaryOp::Eq,
                    left: tag,
                    right: tag_id,
                    operand_ty: i32_ty.clone(),
                    result_ty: Type::Bool,
                });
                let next = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::Branch {
                        condition: matches,
                        then_block: variant_block,
                        else_block: next,
                    },
                );
                Some(next)
            };
            self.current_block = variant_block;
            let tag_text = self.emit(Instruction::String(variant.tag.clone()));
            let tag_node = self.emit_json_host_call(
                JSON_PACK_STRING_HOST,
                vec![tag_text],
                i32_ty.clone(),
            )?;
            let tag_key = self.emit(Instruction::String("tag".to_string()));
            self.emit_json_host_call(
                JSON_PACK_OBJECT_SET_HOST,
                vec![object, tag_key, tag_node],
                Type::Unit,
            )?;
            let payload_ty = variant.payload.as_ref();
            let unboxed = self.emit(Instruction::Cast {
                value: payload,
                from: Type::Unknown,
                to: to_runtime_type(payload_ty),
            });
            let payload_node = self.lower_json_pack_value(unboxed, payload_ty)?;
            let value_key = self.emit(Instruction::String("value".to_string()));
            self.emit_json_host_call(
                JSON_PACK_OBJECT_SET_HOST,
                vec![object, value_key, payload_node],
                Type::Unit,
            )?;
            let exit = self.current_block;
            self.set_terminator(exit, Terminator::Jump(merge));
            if let Some(next) = next_dispatch {
                self.current_block = next;
            }
        }
        self.current_block = merge;
        Ok(object)
    }

    fn lower_json_unpack_value(
        &mut self,
        node: ValueId,
        ty: &Type,
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        match ty {
            Type::Opaque { ty, .. } => {
                self.lower_json_unpack_value(node, ty)
            }
            Type::Numeric(numeric) => self.emit_json_host_call(
                json_unpack_numeric_host(*numeric),
                vec![node],
                Type::Numeric(*numeric),
            ),
            Type::Bool => {
                self.emit_json_host_call(JSON_UNPACK_BOOL_HOST, vec![node], Type::Bool)
            }
            Type::String | Type::StringLiteralUnion(_) => {
                self.emit_json_host_call(JSON_UNPACK_STRING_HOST, vec![node], Type::String)
            }
            Type::Bytes => {
                self.emit_json_host_call(JSON_UNPACK_BYTES_HOST, vec![node], Type::Bytes)
            }
            Type::Unit => Ok(self.emit(Instruction::Unit)),
            Type::Nullable(inner) => {
                let nullable_runtime_ty = to_runtime_type(ty);
                let is_null = self.emit_json_host_call(
                    JSON_UNPACK_IS_NULL_HOST,
                    vec![node],
                    Type::Bool,
                )?;
                let null_block = self.new_block();
                let value_block = self.new_block();
                let merge_block = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::Branch {
                        condition: is_null,
                        then_block: null_block,
                        else_block: value_block,
                    },
                );
                self.current_block = null_block;
                let null = self.emit(Instruction::Null {
                    ty: nullable_runtime_ty.clone(),
                });
                self.set_terminator(null_block, Terminator::Jump(merge_block));
                self.current_block = value_block;
                let decoded = self.lower_json_unpack_value(node, inner)?;
                let widened = self.emit(Instruction::Cast {
                    value: decoded,
                    from: to_runtime_type(inner),
                    to: nullable_runtime_ty,
                });
                let value_exit = self.current_block;
                self.set_terminator(value_exit, Terminator::Jump(merge_block));
                self.current_block = merge_block;
                Ok(self.emit(Instruction::Phi(vec![
                    (null_block, null),
                    (value_exit, widened),
                ])))
            }
            Type::Array(element) => {
                let len = self.emit_json_host_call(
                    JSON_UNPACK_ARRAY_LEN_HOST,
                    vec![node],
                    i32_ty.clone(),
                )?;
                let array = self.emit(Instruction::ArrayNew {
                    element_ty: to_runtime_type(element),
                    elements: Vec::new(),
                });
                self.lower_json_unpack_sequence_loop(node, len, |builder, child| {
                    builder.lower_json_unpack_value(child, element)
                }, |builder, index, value| {
                    builder.emit(Instruction::ArraySet {
                        array,
                        index,
                        value,
                        element_ty: to_runtime_type(element),
                    });
                    Ok(())
                })?;
                Ok(array)
            }
            Type::TypedArray(kind) => {
                let len = self.emit_json_host_call(
                    JSON_UNPACK_ARRAY_LEN_HOST,
                    vec![node],
                    i32_ty.clone(),
                )?;
                let buffer = self.emit(Instruction::BufferNewSized { kind: *kind, len });
                let element_ty = Type::Numeric(kind.element_numeric_type());
                self.lower_json_unpack_sequence_loop(node, len, |builder, child| {
                    builder.lower_json_unpack_value(child, &element_ty)
                }, |builder, index, value| {
                    builder.emit(Instruction::BufferSet {
                        buffer,
                        index,
                        value,
                        kind: *kind,
                    });
                    Ok(())
                })?;
                Ok(buffer)
            }
            Type::Record(fields) => {
                let mut values = Vec::with_capacity(fields.len());
                for (name, field_ty) in fields.iter() {
                    let key = self.emit(Instruction::String(name.clone()));
                    let child = self.emit_json_host_call(
                        JSON_UNPACK_OBJECT_GET_HOST,
                        vec![node, key],
                        i32_ty.clone(),
                    )?;
                    values.push(self.lower_json_unpack_value(child, field_ty)?);
                }
                Ok(self.emit(Instruction::StructNew {
                    struct_ty: to_runtime_type(ty),
                    fields: values,
                }))
            }
            Type::TaggedVariant(variant) => {
                self.lower_json_unpack_tagged(node, std::slice::from_ref(variant))
            }
            Type::TaggedUnion(variants) => self.lower_json_unpack_tagged(node, variants),
            other => Err(Diagnostic::new(format!(
                "json.unpack does not support values of type {other}"
            ))),
        }
    }

    fn lower_json_unpack_sequence_loop(
        &mut self,
        node: ValueId,
        len: ValueId,
        mut decode: impl FnMut(&mut Self, ValueId) -> Result<ValueId, Diagnostic>,
        mut store: impl FnMut(&mut Self, ValueId, ValueId) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        let zero = self.emit_i32_const(0);
        let one = self.emit_i32_const(1);
        let entry = self.current_block;
        let header = self.new_block();
        let body = self.new_block();
        let done = self.new_block();
        self.set_terminator(entry, Terminator::Jump(header));
        self.current_block = header;
        let index = self.emit(Instruction::Phi(vec![(entry, zero)]));
        let in_bounds = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index,
            right: len,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: in_bounds,
                then_block: body,
                else_block: done,
            },
        );
        self.current_block = body;
        let child = self.emit_json_host_call(
            JSON_UNPACK_ARRAY_GET_HOST,
            vec![node, index],
            i32_ty.clone(),
        )?;
        let value = decode(self, child)?;
        store(self, index, value)?;
        let next = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: index,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty,
        });
        let body_exit = self.current_block;
        self.set_terminator(body_exit, Terminator::Jump(header));
        add_phi_incoming(&mut self.function, header, index, (body_exit, next));
        self.current_block = done;
        Ok(())
    }

    fn lower_json_unpack_tagged(
        &mut self,
        node: ValueId,
        variants: &[TaggedVariant],
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        let merge = self.new_block();
        let mut incoming = Vec::with_capacity(variants.len());
        for (index, variant) in variants.iter().enumerate() {
            let variant_block = self.new_block();
            let next_dispatch = if index + 1 == variants.len() {
                self.set_terminator(self.current_block, Terminator::Jump(variant_block));
                None
            } else {
                let tag = self.emit(Instruction::String(variant.tag.clone()));
                let matches = self.emit_json_host_call(
                    JSON_UNPACK_VARIANT_IS_HOST,
                    vec![node, tag],
                    Type::Bool,
                )?;
                let next = self.new_block();
                self.set_terminator(
                    self.current_block,
                    Terminator::Branch {
                        condition: matches,
                        then_block: variant_block,
                        else_block: next,
                    },
                );
                Some(next)
            };
            self.current_block = variant_block;
            let value_key = self.emit(Instruction::String("value".to_string()));
            let payload_node = self.emit_json_host_call(
                JSON_UNPACK_OBJECT_GET_HOST,
                vec![node, value_key],
                i32_ty.clone(),
            )?;
            let payload_ty = variant.payload.as_ref();
            let payload = self.lower_json_unpack_value(payload_node, payload_ty)?;
            let boxed = self.emit(Instruction::Cast {
                value: payload,
                from: to_runtime_type(payload_ty),
                to: Type::Unknown,
            });
            let tag = self.emit_i32_const(self.variant_tag_id(&variant.tag)?);
            let tagged = self.emit(Instruction::StructNew {
                struct_ty: Type::canonical_tagged_union_record(),
                fields: vec![tag, boxed],
            });
            let exit = self.current_block;
            self.set_terminator(exit, Terminator::Jump(merge));
            incoming.push((exit, tagged));
            if let Some(next) = next_dispatch {
                self.current_block = next;
            }
        }
        self.current_block = merge;
        Ok(self.emit(Instruction::Phi(incoming)))
    }

    fn lower_string_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Option<Result<ValueId, Diagnostic>> {
        let i32_ty = Type::Numeric(NumericType::I32);
        match name {
            STRING_FIND | STRING_MATCH | STRING_GSUB | STRING_GMATCH => {
                return Some(self.lower_pattern_builtin(name, args, scope, expected));
            }
            _ => {}
        }
        let lowered = match name {
            STRING_LEN => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_LEN} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let value = match self.lower_expr(&args[0], scope, Some(Type::String)) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                self.emit_string_host_call(STRING_LEN_HOST, vec![value], i32_ty.clone())
            }
            STRING_SUB => {
                if args.len() < 2 || args.len() > 3 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_SUB} expects 2 or 3 arguments, got {}",
                        args.len()
                    ))));
                }
                let value = match self.lower_expr(&args[0], scope, Some(Type::String)) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                let first = match self.lower_expr(&args[1], scope, Some(i32_ty.clone())) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                let last = match args.get(2) {
                    Some(arg) => match self.lower_expr(arg, scope, Some(i32_ty.clone())) {
                        Ok(val) => val,
                        Err(error) => return Some(Err(error)),
                    },
                    None => self.emit_i32_const(-1),
                };
                self.emit_string_host_call(
                    STRING_SUB_HOST,
                    vec![value, first, last],
                    Type::String,
                )
            }
            STRING_REP => {
                if args.len() < 2 || args.len() > 3 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_REP} expects 2 or 3 arguments, got {}",
                        args.len()
                    ))));
                }
                let value = match self.lower_expr(&args[0], scope, Some(Type::String)) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                let count = match self.lower_expr(&args[1], scope, Some(i32_ty.clone())) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                let separator = match args.get(2) {
                    Some(arg) => match self.lower_expr(arg, scope, Some(Type::String)) {
                        Ok(val) => val,
                        Err(error) => return Some(Err(error)),
                    },
                    None => self.emit(Instruction::String(String::new())),
                };
                self.emit_string_host_call(
                    STRING_REP_HOST,
                    vec![value, count, separator],
                    Type::String,
                )
            }
            STRING_BYTE => {
                if args.is_empty() || args.len() > 3 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_BYTE} expects 1 to 3 arguments, got {}",
                        args.len()
                    ))));
                }
                let value = match self.lower_expr(&args[0], scope, Some(Type::String)) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                if args.len() == 3 {
                    let expected_count = expected.as_ref().and_then(|ty| match ty {
                        Type::Multi(tys) => Some(tys.len()),
                        _ => None,
                    });
                    let indices = if let Some(count) = expected_count {
                        // The fixed IR tuple shape represents the values consumed by
                        // the surrounding context. Evaluate both bounds once, then
                        // materialize that prefix from the runtime start index.
                        let first = match self.lower_expr(
                            &args[1],
                            scope,
                            Some(i32_ty.clone()),
                        ) {
                            Ok(first) => first,
                            Err(error) => return Some(Err(error)),
                        };
                        if let Err(error) =
                            self.lower_expr(&args[2], scope, Some(i32_ty.clone()))
                        {
                            return Some(Err(error));
                        }
                        (0..count)
                            .map(|offset| {
                                if offset == 0 {
                                    first
                                } else {
                                    let offset = self.emit_i32_const(offset as i32);
                                    self.emit(Instruction::Binary {
                                        op: BinaryOp::Add,
                                        left: first,
                                        right: offset,
                                        operand_ty: i32_ty.clone(),
                                        result_ty: i32_ty.clone(),
                                    })
                                }
                            })
                            .collect::<Vec<_>>()
                    } else {
                        let indices = match string_byte_static_indices(args, expected.as_ref()) {
                            Ok(indices) => indices,
                            Err(error) => return Some(Err(error)),
                        };
                        indices
                            .into_iter()
                            .map(|index| self.emit_i32_const(index))
                            .collect::<Vec<_>>()
                    };
                    let mut values = Vec::with_capacity(indices.len());
                    for index in indices {
                        match self.emit_string_host_call(
                            STRING_BYTE_HOST,
                            vec![value, index],
                            i32_ty.clone(),
                        ) {
                            Ok(byte) => values.push(byte),
                            Err(error) => return Some(Err(error)),
                        }
                    }
                    let pack_types = vec![i32_ty.clone(); values.len()];
                    Ok(self.emit(Instruction::PackMulti {
                        values,
                        types: pack_types,
                    }))
                } else {
                    let index = match args.get(1) {
                        Some(arg) => match self.lower_expr(arg, scope, Some(i32_ty.clone())) {
                            Ok(val) => val,
                            Err(error) => return Some(Err(error)),
                        },
                        None => self.emit_i32_const(1),
                    };
                    let raw = match self.emit_string_host_call(
                        STRING_BYTE_HOST,
                        vec![value, index],
                        i32_ty.clone(),
                    ) {
                        Ok(raw) => raw,
                        Err(error) => return Some(Err(error)),
                    };
                    let nullable = self.lower_nullable_i32_sentinel(raw, -1);
                    if string_byte_scalar_known_in_range(args)
                        || string_byte_requires_numeric_value(expected.as_ref())
                    {
                        Ok(self.emit(Instruction::Cast {
                            value: nullable,
                            from: Type::Nullable(Arc::new(i32_ty.clone())),
                            to: i32_ty.clone(),
                        }))
                    } else {
                        Ok(nullable)
                    }
                }
            }
            STRING_REVERSE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_REVERSE} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let value = match self.lower_expr(&args[0], scope, Some(Type::String)) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                self.emit_string_host_call(STRING_REVERSE_HOST, vec![value], Type::String)
            }
            STRING_UPPER | STRING_LOWER => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{name} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                let value = match self.lower_expr(&args[0], scope, Some(Type::String)) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                let host_name = if name == STRING_UPPER {
                    STRING_UPPER_HOST
                } else {
                    STRING_LOWER_HOST
                };
                self.emit_string_host_call(host_name, vec![value], Type::String)
            }
            STRING_CHAR => {
                let mut lowered_args = Vec::new();
                for (arg_index, arg) in args.iter().enumerate() {
                    let is_last = arg_index + 1 == args.len();
                    // Calls are inferred without an expectation first so a
                    // multi-value result stays visible; an i32 expectation
                    // would collapse it to its first value before the
                    // adjustment rule below can see it.
                    let is_call = matches!(arg, Expr::Call { .. } | Expr::MethodCall { .. });
                    let arg_ty = if is_call {
                        match self.infer_expr_type(arg, &scope.types, None) {
                            Ok(ty) => ty,
                            Err(_) => match self.infer_expr_type(arg, &scope.types, Some(i32_ty.clone())) {
                                Ok(ty) => ty,
                                Err(error) => return Some(Err(error)),
                            },
                        }
                    } else {
                        match self.infer_expr_type(arg, &scope.types, Some(i32_ty.clone())) {
                            Ok(ty) => ty,
                            Err(_) => match self.infer_expr_type(arg, &scope.types, None) {
                                Ok(ty) => ty,
                                Err(error) => return Some(Err(error)),
                            },
                        }
                    };
                    match arg_ty {
                        Type::Multi(multi_types) => {
                            let tuple = match self.lower_expr(arg, scope, None) {
                                Ok(value) => value,
                                Err(error) => return Some(Err(error)),
                            };
                            // Only the last argument expands its multi-value
                            // result; earlier ones adjust to their first value.
                            let expanded = if is_last {
                                multi_types.len()
                            } else {
                                1.min(multi_types.len())
                            };
                            for (index, part) in multi_types.into_iter().take(expanded).enumerate() {
                                if part != i32_ty {
                                    return Some(Err(Diagnostic::new(format!(
                                        "{STRING_CHAR} expects i32 character codes, got {part}"
                                    ))));
                                }
                                lowered_args.push(self.emit(Instruction::MultiGet {
                                    value: tuple,
                                    index,
                                    ty: i32_ty.clone(),
                                }));
                            }
                        }
                        ty => {
                            if ty != i32_ty {
                                return Some(Err(Diagnostic::new(format!(
                                    "{STRING_CHAR} expects i32 character codes, got {ty}"
                                ))));
                            }
                            match self.lower_expr(arg, scope, Some(i32_ty.clone())) {
                                Ok(value) => lowered_args.push(value),
                                Err(error) => return Some(Err(error)),
                            }
                        }
                    }
                }
                if lowered_args.len() > 16 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_CHAR} expects at most 16 statically expanded arguments, got {}",
                        lowered_args.len()
                    ))));
                }
                let host_name = format!("{STRING_CHAR_HOST_PREFIX}{}", lowered_args.len());
                self.emit_string_host_call(&host_name, lowered_args, Type::String)
            }
            STRING_FORMAT => {
                if args.is_empty() || args.len() > 101 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_FORMAT} expects 1 to 101 arguments, got {}",
                        args.len()
                    ))));
                }
                let mut lowered_args = Vec::with_capacity(args.len());
                match self.lower_expr(&args[0], scope, Some(Type::String)) {
                    Ok(val) => lowered_args.push(val),
                    Err(error) => return Some(Err(error)),
                }
                for (arg_index, arg) in args.iter().enumerate().skip(1) {
                    let is_last = arg_index + 1 == args.len();
                    let arg_ty = match self.infer_expr_type(arg, &scope.types, None) {
                        Ok(ty) => ty,
                        Err(error) => return Some(Err(error)),
                    };
                    match arg_ty {
                        Type::Multi(multi_types) => {
                            let tuple = match self.lower_expr(arg, scope, None) {
                                Ok(value) => value,
                                Err(error) => return Some(Err(error)),
                            };
                            // Only the last argument expands its multi-value
                            // result; earlier ones adjust to their first value.
                            let expanded = if is_last {
                                multi_types.len()
                            } else {
                                1.min(multi_types.len())
                            };
                            for (index, part) in multi_types.into_iter().take(expanded).enumerate() {
                                if !(part.is_numeric()
                                    || part == Type::Bool
                                    || part == Type::String
                                    || part == Type::Unknown)
                                {
                                    return Some(Err(Diagnostic::new(format!(
                                        "{STRING_FORMAT} expects primitive format arguments, got {part}",
                                    ))));
                                }
                                let value = self.emit(Instruction::MultiGet {
                                    value: tuple,
                                    index,
                                    ty: part.clone(),
                                });
                                if part == Type::String {
                                    lowered_args.push(value);
                                } else {
                                    lowered_args.push(self.emit(Instruction::ToString {
                                        value,
                                        from: part,
                                    }));
                                }
                            }
                        }
                        _ => match self.lower_expr_to_string(arg, scope) {
                            Ok(val) => lowered_args.push(val),
                            Err(error) => return Some(Err(error)),
                        },
                    }
                }
                if lowered_args.len() > 101 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_FORMAT} expects at most 100 statically expanded format arguments, got {}",
                        lowered_args.len() - 1
                    ))));
                }
                let host_name = format!("{STRING_FORMAT_HOST_PREFIX}{}", lowered_args.len() - 1);
                self.emit_string_host_call(&host_name, lowered_args, Type::String)
            }
            STRING_SPLIT => {
                if args.is_empty() || args.len() > 2 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_SPLIT} expects 1 or 2 arguments, got {}",
                        args.len()
                    ))));
                }
                let source = match self.lower_expr(&args[0], scope, Some(Type::String)) {
                    Ok(val) => val,
                    Err(error) => return Some(Err(error)),
                };
                let separator = match args.get(1) {
                    Some(arg) => match self.lower_expr(arg, scope, Some(Type::String)) {
                        Ok(val) => val,
                        Err(error) => return Some(Err(error)),
                    },
                    // Luau's default separator is ",".
                    None => self.emit(Instruction::String(",".to_string())),
                };
                self.lower_string_split(source, separator)
            }
            _ => return None,
        };

        let (value, result_ty) = match lowered {
            Ok(value) => {
                let result_ty = match name {
                    STRING_LEN => i32_ty,
                    STRING_BYTE if args.len() == 3 => Type::Multi(vec![
                        i32_ty.clone();
                        string_byte_static_count(args, expected.as_ref()).unwrap_or(0)
                    ]),
                    STRING_BYTE
                        if string_byte_scalar_known_in_range(args)
                            || string_byte_requires_numeric_value(expected.as_ref()) =>
                    {
                        i32_ty
                    }
                    STRING_BYTE => Type::Nullable(Arc::new(i32_ty)),
                    STRING_SPLIT => Type::Array(Arc::new(Type::String)),
                    _ => Type::String,
                };
                (value, result_ty)
            }
            Err(error) => return Some(Err(error)),
        };
        Some(self.coerce_value(value, result_ty, expected))
    }

    /// Lower `string.split(source, separator)`: the host computes the pieces
    /// and returns their count; a compiler-emitted loop then reads each piece
    /// back and appends it to a fresh growable `{string}` array. The fill
    /// loop runs immediately after the split host call with no intervening
    /// user code, so the host only needs one pending-split slot.
    fn lower_string_split(
        &mut self,
        source: ValueId,
        separator: ValueId,
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        let count =
            self.emit_string_host_call(STRING_SPLIT_HOST, vec![source, separator], i32_ty.clone())?;
        let array = self.emit(Instruction::ArrayNew {
            element_ty: Type::String,
            elements: Vec::new(),
        });
        let zero = self.emit_i32_const(0);
        let one = self.emit_i32_const(1);

        let preheader = self.current_block;
        let header = self.new_block();
        let body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(preheader, Terminator::Jump(header));

        self.current_block = header;
        let index_phi = self.emit(Instruction::Phi(vec![(preheader, zero)]));
        // Loop-invariant, threaded through a phi with a `+ 0` self-edge so the
        // local allocator treats it as live across the back edge (see
        // emit_array_append_all).
        let count_phi = self.emit(Instruction::Phi(vec![(preheader, count)]));
        let cond = self.emit(Instruction::Binary {
            op: BinaryOp::Less,
            left: index_phi,
            right: count_phi,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            header,
            Terminator::Branch {
                condition: cond,
                then_block: body,
                else_block: exit,
            },
        );

        self.current_block = body;
        let piece =
            self.emit_string_host_call(STRING_SPLIT_GET_HOST, vec![index_phi], Type::String)?;
        let array_len = self.emit(Instruction::ArrayLen { array });
        self.emit(Instruction::ArraySet {
            array,
            index: array_len,
            value: piece,
            element_ty: Type::String,
        });
        let next_index = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: index_phi,
            right: one,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty.clone(),
        });
        let next_count = self.emit(Instruction::Binary {
            op: BinaryOp::Add,
            left: count_phi,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: i32_ty,
        });
        let body_exit = self.current_block;
        self.set_terminator(body_exit, Terminator::Jump(header));
        add_phi_incoming(
            &mut self.function,
            header,
            index_phi,
            (body_exit, next_index),
        );
        add_phi_incoming(
            &mut self.function,
            header,
            count_phi,
            (body_exit, next_count),
        );
        self.current_block = exit;
        Ok(array)
    }

    /// `a or b` where `a: T?` supplies `b` as its default. The left operand is
    /// evaluated once and branched on nil: present means the unboxed left
    /// value, nil means the fallback. Only nil takes the fallback -- this
    /// never looks at truthiness the way Lua's `or` does.
    fn lower_nullable_or(
        &mut self,
        left: &Expr,
        right: &Expr,
        nullable_ty: Type,
        result_ty: Type,
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let inner = nullable_ty
            .nullable_inner()
            .expect("caller checked the nullable left operand");
        let left_value = self.lower_expr(left, scope, Some(nullable_ty.clone()))?;
        let is_null = self.emit(Instruction::IsNull {
            value: left_value,
            ty: inner.clone(),
        });
        let fallback_block = self.new_block();
        let present_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition: is_null,
                then_block: fallback_block,
                else_block: present_block,
            },
        );

        self.current_block = present_block;
        // A `T?` result keeps the left value as-is; a `T` result unboxes it,
        // which is a no-op for reference-typed nullables and unwraps the
        // payload struct for boxed ones.
        let present_value = if result_ty == nullable_ty {
            left_value
        } else {
            let unboxed = self.emit(Instruction::Cast {
                value: left_value,
                from: nullable_ty,
                to: inner.clone(),
            });
            self.coerce_value(unboxed, inner, Some(result_ty.clone()))?
        };
        let present_exit = self.current_block;
        self.set_terminator(present_exit, Terminator::Jump(merge_block));

        self.current_block = fallback_block;
        let fallback_value = self.lower_expr(right, scope, Some(result_ty.clone()))?;
        let fallback_exit = self.current_block;
        self.set_terminator(fallback_exit, Terminator::Jump(merge_block));

        self.current_block = merge_block;
        let mut incoming = vec![
            (present_exit, present_value),
            (fallback_exit, fallback_value),
        ];
        incoming.sort_by_key(|(pred, _)| *pred);
        let value = self.emit(Instruction::Phi(incoming));
        self.coerce_value(value, result_ty, expected)
    }

    /// Equality between a nullable value and a value of its inner type (or
    /// two reference-typed nullables). Reference-typed nullables share their
    /// inner wasm representation and use the null-safe host equality; boxed
    /// nullables (`i32?` etc.) branch on nil before unboxing.
    fn lower_nullable_equality(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        nullable_ty: Type,
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let inner = nullable_ty
            .nullable_inner()
            .expect("caller checked nullable operand");
        if !nullable_ty.is_boxed_nullable() {
            let left_value = self.lower_expr(left, scope, Some(nullable_ty.clone()))?;
            let right_value = self.lower_expr(right, scope, Some(nullable_ty))?;
            let value = self.emit(Instruction::Binary {
                op,
                left: left_value,
                right: right_value,
                operand_ty: inner,
                result_ty: Type::Bool,
            });
            return self.coerce_value(value, Type::Bool, expected);
        }

        let side_is_nullable = |builder: &Self, side: &Expr| {
            builder
                .infer_expr_type(side, &scope.types, None)
                .map(first_of_multi)
                .map(|ty| matches!(ty, Type::Nullable(_)))
                .unwrap_or(false)
        };
        let left_nullable = side_is_nullable(self, left);
        let right_nullable = side_is_nullable(self, right);
        if left_nullable && right_nullable {
            return self.lower_boxed_nullable_pair_equality(
                op,
                left,
                right,
                nullable_ty,
                scope,
                expected,
            );
        }
        let left_expected = if left_nullable {
            nullable_ty.clone()
        } else {
            inner.clone()
        };
        let right_expected = if left_nullable {
            inner.clone()
        } else {
            nullable_ty.clone()
        };
        let left_value = self.lower_expr(left, scope, Some(left_expected))?;
        let right_value = self.lower_expr(right, scope, Some(right_expected))?;
        let (boxed, plain) = if left_nullable {
            (left_value, right_value)
        } else {
            (right_value, left_value)
        };

        let is_null = self.emit(Instruction::IsNull {
            value: boxed,
            ty: inner.clone(),
        });
        let null_block = self.new_block();
        let value_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition: is_null,
                then_block: null_block,
                else_block: value_block,
            },
        );

        self.current_block = null_block;
        // nil == x is false (x is non-nil here); nil ~= x is true.
        let null_result = self.emit(Instruction::Bool(matches!(op, BinaryOp::NotEq)));
        let null_exit = self.current_block;
        self.set_terminator(null_exit, Terminator::Jump(merge_block));

        self.current_block = value_block;
        let unboxed = self.emit(Instruction::Cast {
            value: boxed,
            from: nullable_ty,
            to: inner.clone(),
        });
        let value_result = self.emit(Instruction::Binary {
            op,
            left: unboxed,
            right: plain,
            operand_ty: inner,
            result_ty: Type::Bool,
        });
        let value_exit = self.current_block;
        self.set_terminator(value_exit, Terminator::Jump(merge_block));

        self.current_block = merge_block;
        let mut incoming = vec![(null_exit, null_result), (value_exit, value_result)];
        incoming.sort_by_key(|(pred, _)| *pred);
        let value = self.emit(Instruction::Phi(incoming));
        self.coerce_value(value, Type::Bool, expected)
    }

    /// Equality between two boxed nullables of the same inner type (`i32? ==
    /// i32?`). Lua semantics: `nil == nil` is true, `nil == value` is false,
    /// and two present values compare by their unboxed contents. Lowered as
    /// nested diamonds so the unboxing casts only run where both sides are
    /// known to be non-nil.
    fn lower_boxed_nullable_pair_equality(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        nullable_ty: Type,
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let inner = nullable_ty
            .nullable_inner()
            .expect("caller checked nullable operand");
        // i64/u64 have no `unknown` boxing yet (see waluau-agmp), so their
        // nullables cannot be unboxed for the value-vs-value arm.
        if !matches!(
            inner,
            Type::Bool
                | Type::Numeric(
                    NumericType::I32
                        | NumericType::U32
                        | NumericType::F32
                        | NumericType::F64,
                )
        ) {
            return Err(Diagnostic::new(format!(
                "comparing two nullable {inner} values is not supported yet",
            )));
        }
        let is_eq = matches!(op, BinaryOp::Eq);

        let left_value = self.lower_expr(left, scope, Some(nullable_ty.clone()))?;
        let right_value = self.lower_expr(right, scope, Some(nullable_ty.clone()))?;
        let left_is_null = self.emit(Instruction::IsNull {
            value: left_value,
            ty: inner.clone(),
        });
        let right_is_null = self.emit(Instruction::IsNull {
            value: right_value,
            ty: inner.clone(),
        });

        let left_null_block = self.new_block();
        let left_value_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition: left_is_null,
                then_block: left_null_block,
                else_block: left_value_block,
            },
        );

        // left is nil: equal exactly when right is nil too.
        self.current_block = left_null_block;
        let left_null_result = if is_eq {
            right_is_null
        } else {
            let false_value = self.emit(Instruction::Bool(false));
            self.emit(Instruction::Binary {
                op: BinaryOp::Eq,
                left: right_is_null,
                right: false_value,
                operand_ty: Type::Bool,
                result_ty: Type::Bool,
            })
        };
        let left_null_exit = self.current_block;
        self.set_terminator(left_null_exit, Terminator::Jump(merge_block));

        // left is present: nil on the right is never equal, otherwise unbox both.
        self.current_block = left_value_block;
        let right_null_block = self.new_block();
        let both_present_block = self.new_block();
        let inner_merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition: right_is_null,
                then_block: right_null_block,
                else_block: both_present_block,
            },
        );

        self.current_block = right_null_block;
        let right_null_result = self.emit(Instruction::Bool(!is_eq));
        let right_null_exit = self.current_block;
        self.set_terminator(right_null_exit, Terminator::Jump(inner_merge_block));

        self.current_block = both_present_block;
        let left_unboxed = self.emit(Instruction::Cast {
            value: left_value,
            from: nullable_ty.clone(),
            to: inner.clone(),
        });
        let right_unboxed = self.emit(Instruction::Cast {
            value: right_value,
            from: nullable_ty,
            to: inner.clone(),
        });
        let both_present_result = self.emit(Instruction::Binary {
            op,
            left: left_unboxed,
            right: right_unboxed,
            operand_ty: inner,
            result_ty: Type::Bool,
        });
        let both_present_exit = self.current_block;
        self.set_terminator(both_present_exit, Terminator::Jump(inner_merge_block));

        self.current_block = inner_merge_block;
        let mut inner_incoming = vec![
            (right_null_exit, right_null_result),
            (both_present_exit, both_present_result),
        ];
        inner_incoming.sort_by_key(|(pred, _)| *pred);
        let inner_result = self.emit(Instruction::Phi(inner_incoming));
        let inner_merge_exit = self.current_block;
        self.set_terminator(inner_merge_exit, Terminator::Jump(merge_block));

        self.current_block = merge_block;
        let mut incoming = vec![
            (left_null_exit, left_null_result),
            (inner_merge_exit, inner_result),
        ];
        incoming.sort_by_key(|(pred, _)| *pred);
        let value = self.emit(Instruction::Phi(incoming));
        self.coerce_value(value, Type::Bool, expected)
    }

    /// Static capture kinds for `string.find`'s pattern argument. A literal
    /// `plain = true` disables pattern interpretation entirely.
    fn find_pattern_captures(args: &[Expr]) -> Vec<waluau_ast::LuaCaptureKind> {
        if matches!(args.get(3), Some(Expr::Bool(true, _))) {
            return Vec::new();
        }
        args.get(1)
            .map(waluau_ast::expr_pattern_captures)
            .unwrap_or_default()
    }

    /// Lua-pattern builtins (`string.find`/`string.match`/`string.gsub`).
    /// Follows the pcall multi-value convention: with an absent or Multi
    /// expected type the full PackMulti tuple is produced; with a single
    /// expected type only the first result is materialized and coerced.
    fn lower_pattern_builtin(
        &mut self,
        name: &str,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        match name {
            STRING_FIND => self.lower_string_find(args, scope, expected),
            STRING_MATCH => self.lower_string_match(args, scope, expected),
            STRING_GSUB => self.lower_string_gsub(args, scope, expected),
            _ => Err(Diagnostic::new(format!(
                "{STRING_GMATCH} is only supported as a for-in iterator"
            ))),
        }
    }

    fn lower_string_find(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        if args.len() < 2 || args.len() > 4 {
            return Err(Diagnostic::new(format!(
                "{STRING_FIND} expects 2 to 4 arguments, got {}",
                args.len()
            )));
        }
        let haystack = self.lower_expr(&args[0], scope, Some(Type::String))?;
        let pattern = self.lower_expr(&args[1], scope, Some(Type::String))?;
        let init = match args.get(2) {
            Some(arg) => self.lower_expr(arg, scope, Some(i32_ty.clone()))?,
            None => self.emit_i32_const(1),
        };
        let plain = match args.get(3) {
            Some(arg) => self.lower_expr(arg, scope, Some(Type::Bool))?,
            None => self.emit(Instruction::Bool(false)),
        };
        let matched = self.emit_string_host_call(
            PM_FIND_HOST,
            vec![haystack, pattern, init, plain],
            i32_ty,
        )?;
        let mut slots = vec![PmSlot::Start, PmSlot::End];
        for (index, kind) in Self::find_pattern_captures(args).iter().enumerate() {
            slots.push(PmSlot::capture(*kind, index as i32 + 1, false));
        }
        self.finish_pm_call(matched, slots, expected)
    }

    fn lower_string_match(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        if args.len() < 2 || args.len() > 3 {
            return Err(Diagnostic::new(format!(
                "{STRING_MATCH} expects 2 or 3 arguments, got {}",
                args.len()
            )));
        }
        let haystack = self.lower_expr(&args[0], scope, Some(Type::String))?;
        let pattern = self.lower_expr(&args[1], scope, Some(Type::String))?;
        let init = match args.get(2) {
            Some(arg) => self.lower_expr(arg, scope, Some(i32_ty.clone()))?,
            None => self.emit_i32_const(1),
        };
        let matched =
            self.emit_string_host_call(PM_MATCH_HOST, vec![haystack, pattern, init], i32_ty)?;
        let captures = args
            .get(1)
            .map(waluau_ast::expr_pattern_captures)
            .unwrap_or_default();
        let slots = if captures.is_empty() {
            vec![PmSlot::capture(waluau_ast::LuaCaptureKind::Str, 0, true)]
        } else {
            captures
                .iter()
                .enumerate()
                .map(|(index, kind)| PmSlot::capture(*kind, index as i32 + 1, true))
                .collect()
        };
        self.finish_pm_call(matched, slots, expected)
    }

    /// Emits the matched/unmatched diamond around a `pm_find`/`pm_match`
    /// result and packs the requested result slots.
    fn finish_pm_call(
        &mut self,
        matched: ValueId,
        slots: Vec<PmSlot>,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let want_multi = matches!(expected, None | Some(Type::Multi(_)));
        let slots = if want_multi {
            slots
        } else {
            slots.into_iter().take(1).collect()
        };
        let values = self.lower_pm_result_slots(matched, &slots)?;
        if want_multi {
            let (vals, tys): (Vec<_>, Vec<_>) = values.into_iter().unzip();
            let packed = self.emit(Instruction::PackMulti {
                values: vals,
                types: tys.clone(),
            });
            self.coerce_value(packed, Type::Multi(tys), expected)
        } else {
            let (value, ty) = values.into_iter().next().expect("at least one pm slot");
            self.coerce_value(value, ty, expected)
        }
    }

    /// Branches on `matched != 0` and produces one value per slot: the host
    /// accessor's value in the matched branch, the slot's miss value (nil,
    /// zero, or the empty string) otherwise, merged with phis.
    fn lower_pm_result_slots(
        &mut self,
        matched: ValueId,
        slots: &[PmSlot],
    ) -> Result<Vec<(ValueId, Type)>, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        let zero = self.emit_i32_const(0);
        let condition = self.emit(Instruction::Binary {
            op: BinaryOp::NotEq,
            left: matched,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        let then_block = self.new_block();
        let else_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            },
        );

        self.current_block = then_block;
        let mut then_values = Vec::with_capacity(slots.len());
        for slot in slots {
            then_values.push(self.lower_pm_slot_hit(slot)?);
        }
        let then_exit = self.current_block;
        self.set_terminator(then_exit, Terminator::Jump(merge_block));

        self.current_block = else_block;
        let mut else_values = Vec::with_capacity(slots.len());
        for slot in slots {
            else_values.push(self.lower_pm_slot_miss(slot));
        }
        let else_exit = self.current_block;
        self.set_terminator(else_exit, Terminator::Jump(merge_block));

        self.current_block = merge_block;
        let mut merged = Vec::with_capacity(slots.len());
        for ((slot, then_value), else_value) in slots.iter().zip(then_values).zip(else_values) {
            let mut incoming = vec![(then_exit, then_value), (else_exit, else_value)];
            incoming.sort_by_key(|(pred, _)| *pred);
            merged.push((self.emit(Instruction::Phi(incoming)), slot.value_type()));
        }
        Ok(merged)
    }

    fn lower_pm_slot_hit(&mut self, slot: &PmSlot) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        match slot {
            PmSlot::Start => {
                let raw = self.emit_string_host_call(PM_MATCH_START_HOST, vec![], i32_ty.clone())?;
                Ok(self.emit(Instruction::Cast {
                    value: raw,
                    from: i32_ty.clone(),
                    to: Type::Nullable(Arc::new(i32_ty)),
                }))
            }
            PmSlot::End => self.emit_string_host_call(PM_MATCH_END_HOST, vec![], i32_ty),
            PmSlot::Capture {
                kind,
                index,
                nullable,
            } => {
                let index_value = self.emit_i32_const(*index);
                let (host, raw_ty) = match kind {
                    waluau_ast::LuaCaptureKind::Str => (PM_CAPTURE_STRING_HOST, Type::String),
                    waluau_ast::LuaCaptureKind::Position => {
                        (PM_CAPTURE_POSITION_HOST, i32_ty.clone())
                    }
                };
                let raw = self.emit_string_host_call(host, vec![index_value], raw_ty.clone())?;
                if *nullable {
                    Ok(self.emit(Instruction::Cast {
                        value: raw,
                        from: raw_ty.clone(),
                        to: Type::Nullable(Arc::new(raw_ty)),
                    }))
                } else {
                    Ok(raw)
                }
            }
        }
    }

    fn lower_pm_slot_miss(&mut self, slot: &PmSlot) -> ValueId {
        match slot {
            PmSlot::Start => self.emit(Instruction::Null {
                ty: slot.value_type(),
            }),
            PmSlot::End => self.emit_i32_const(0),
            PmSlot::Capture {
                kind,
                nullable,
                ..
            } => {
                if *nullable {
                    self.emit(Instruction::Null {
                        ty: slot.value_type(),
                    })
                } else {
                    match kind {
                        waluau_ast::LuaCaptureKind::Str => {
                            self.emit(Instruction::String(String::new()))
                        }
                        waluau_ast::LuaCaptureKind::Position => self.emit_i32_const(0),
                    }
                }
            }
        }
    }

    fn lower_string_gsub(
        &mut self,
        args: &[Expr],
        scope: &Scope,
        expected: Option<Type>,
    ) -> Result<ValueId, Diagnostic> {
        let i32_ty = Type::Numeric(NumericType::I32);
        if args.len() < 3 || args.len() > 4 {
            return Err(Diagnostic::new(format!(
                "{STRING_GSUB} expects 3 or 4 arguments, got {}",
                args.len()
            )));
        }
        let source = self.lower_expr(&args[0], scope, Some(Type::String))?;
        let pattern = self.lower_expr(&args[1], scope, Some(Type::String))?;
        let max_count = match args.get(3) {
            Some(arg) => self.lower_expr(arg, scope, Some(i32_ty.clone()))?,
            None => self.emit_i32_const(-1),
        };

        let repl_fn_ty = self.gsub_replacement_function_type(&args[1], &args[2], &scope.types)?;
        let (out, count) = if let Some((param_tys, return_ty)) = repl_fn_ty {
            let repl_ty = Type::Function {
                params: param_tys.clone(),
                return_type: Arc::new(return_ty.clone()),
                has_self: false,
            };
            let repl = self.lower_expr(&args[2], scope, Some(repl_ty))?;
            self.lower_gsub_function_loop(source, pattern, max_count, repl, &args[1], (param_tys, return_ty))?
        } else {
            let repl = self.lower_expr(&args[2], scope, Some(Type::String))?;
            let out = self.emit_string_host_call(
                PM_GSUB_HOST,
                vec![source, pattern, repl, max_count],
                Type::String,
            )?;
            let count = self.emit_string_host_call(PM_GSUB_COUNT_HOST, vec![], i32_ty.clone())?;
            (out, count)
        };

        let want_multi = matches!(expected, None | Some(Type::Multi(_)));
        if want_multi {
            let tys = vec![Type::String, i32_ty];
            let packed = self.emit(Instruction::PackMulti {
                values: vec![out, count],
                types: tys.clone(),
            });
            self.coerce_value(packed, Type::Multi(tys), expected)
        } else {
            self.coerce_value(out, Type::String, expected)
        }
    }

    /// Decides whether the gsub replacement argument is a function and, if
    /// so, resolves its parameter/return types. Returns None for
    /// replacement-string templates.
    fn gsub_replacement_function_type(
        &self,
        pattern_arg: &Expr,
        repl_arg: &Expr,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<Option<(Vec<Type>, Type)>, Diagnostic> {
        let captures = waluau_ast::expr_pattern_captures(pattern_arg);
        let expected_params: Vec<Type> = if captures.is_empty() {
            vec![Type::String]
        } else {
            captures.iter().map(|kind| kind.value_type()).collect()
        };
        if let Ok(ty) = self.infer_expr_type(repl_arg, types, None) {
            return match ty {
                Type::Function {
                    params,
                    return_type,
                    ..
                } => Ok(Some((params, Arc::unwrap_or_clone(return_type)))),
                _ => Ok(None),
            };
        }
        // Unannotated function expressions cannot be inferred without an
        // expected type; try the two supported replacer shapes.
        for return_ty in [Type::String, Type::Unit] {
            let candidate = Type::Function {
                params: expected_params.clone(),
                return_type: Arc::new(return_ty.clone()),
                has_self: false,
            };
            if self
                .infer_expr_type(repl_arg, types, Some(candidate))
                .is_ok()
            {
                return Ok(Some((expected_params, return_ty)));
            }
        }
        // Fall through to the replacement-string path, whose lowering will
        // report the underlying inference error.
        Ok(None)
    }

    /// gsub with a function replacement: the host walks the matches
    /// (pm_gsub_begin/next/finish) while the guest computes each
    /// replacement via an indirect call.
    fn lower_gsub_function_loop(
        &mut self,
        source: ValueId,
        pattern: ValueId,
        max_count: ValueId,
        repl: ValueId,
        pattern_arg: &Expr,
        signature: (Vec<Type>, Type),
    ) -> Result<(ValueId, ValueId), Diagnostic> {
        let (param_tys, return_ty) = signature;
        let i32_ty = Type::Numeric(NumericType::I32);
        let handle = self.emit_string_host_call(
            PM_GSUB_BEGIN_HOST,
            vec![source, pattern, max_count],
            i32_ty.clone(),
        )?;
        let captures = waluau_ast::expr_pattern_captures(pattern_arg);

        let header = self.new_block();
        let body = self.new_block();
        let exit = self.new_block();
        self.set_terminator(self.current_block, Terminator::Jump(header));

        self.current_block = header;
        let has = self.emit_string_host_call(PM_GSUB_NEXT_HOST, vec![handle], i32_ty.clone())?;
        let zero = self.emit_i32_const(0);
        let condition = self.emit(Instruction::Binary {
            op: BinaryOp::NotEq,
            left: has,
            right: zero,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition,
                then_block: body,
                else_block: exit,
            },
        );

        self.current_block = body;
        let mut call_args = Vec::with_capacity(param_tys.len());
        for (index, param_ty) in param_tys.iter().enumerate() {
            let capture_kind = if captures.is_empty() {
                waluau_ast::LuaCaptureKind::Str
            } else {
                *captures.get(index).ok_or_else(|| {
                    Diagnostic::new(format!(
                        "{STRING_GSUB} replacement function takes {} arguments, but the pattern only has {} captures",
                        param_tys.len(),
                        captures.len()
                    ))
                })?
            };
            let capture_index = if captures.is_empty() { 0 } else { index as i32 + 1 };
            let index_value = self.emit_i32_const(capture_index);
            let (host, raw_ty) = match capture_kind {
                waluau_ast::LuaCaptureKind::Str => (PM_CAPTURE_STRING_HOST, Type::String),
                waluau_ast::LuaCaptureKind::Position => (PM_CAPTURE_POSITION_HOST, i32_ty.clone()),
            };
            if *param_ty != raw_ty {
                return Err(Diagnostic::new(format!(
                    "{STRING_GSUB} replacement function argument #{} expects {raw_ty}, got {param_ty}",
                    index + 1
                )));
            }
            call_args.push(self.emit_string_host_call(host, vec![index_value], raw_ty)?);
        }
        let call = self.emit(Instruction::CallValue {
            callee: repl,
            args: call_args,
            params: param_tys.clone(),
            return_type: return_ty.clone(),
        });
        match &return_ty {
            Type::String => {
                self.emit_string_host_call(PM_GSUB_REPLACE_HOST, vec![handle, call], Type::Unit)?;
            }
            Type::Unit => {
                self.emit_string_host_call(PM_GSUB_KEEP_HOST, vec![handle], Type::Unit)?;
            }
            other if other.is_numeric() => {
                let text = self.emit(Instruction::ToString {
                    value: call,
                    from: other.clone(),
                });
                self.emit_string_host_call(PM_GSUB_REPLACE_HOST, vec![handle, text], Type::Unit)?;
            }
            other => {
                return Err(Diagnostic::new(format!(
                    "{STRING_GSUB} replacement function must return a string, a number, or nothing, got {other}"
                )));
            }
        }
        self.set_terminator(self.current_block, Terminator::Jump(header));

        self.current_block = exit;
        let out =
            self.emit_string_host_call(PM_GSUB_FINISH_HOST, vec![handle], Type::String)?;
        let count = self.emit_string_host_call(PM_GSUB_COUNT_HOST, vec![], i32_ty)?;
        Ok((out, count))
    }

    fn infer_string_builtin_call_type(
        &self,
        name: &str,
        args: &[Expr],
        types: &HashMap<SymbolId, Type>,
        expected: Option<Type>,
    ) -> Option<Result<Type, Diagnostic>> {
        let i32_ty = Type::Numeric(NumericType::I32);
        match name {
            STRING_FIND => {
                if args.len() < 2 || args.len() > 4 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_FIND} expects 2 to 4 arguments, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_FIND,
                    "haystack",
                    &args[0],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_FIND,
                    "pattern",
                    &args[1],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                if let Some(init_arg) = args.get(2) {
                    if let Err(error) = self.require_inferred_arg_type(
                        STRING_FIND,
                        "init",
                        init_arg,
                        i32_ty.clone(),
                        types,
                    ) {
                        return Some(Err(error));
                    }
                }
                if let Some(plain_arg) = args.get(3) {
                    if let Err(error) = self.require_inferred_arg_type(
                        STRING_FIND,
                        "plain",
                        plain_arg,
                        Type::Bool,
                        types,
                    ) {
                        return Some(Err(error));
                    }
                }
                Some(Ok(Type::Multi(waluau_ast::string_find_result_types(
                    &Self::find_pattern_captures(args),
                ))))
            }
            STRING_MATCH => {
                if args.len() < 2 || args.len() > 3 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_MATCH} expects 2 or 3 arguments, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_MATCH,
                    "haystack",
                    &args[0],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                if let Some(init_arg) = args.get(2) {
                    if let Err(error) = self.require_inferred_arg_type(
                        STRING_MATCH,
                        "init",
                        init_arg,
                        i32_ty.clone(),
                        types,
                    ) {
                        return Some(Err(error));
                    }
                }
                Some(Ok(Type::Multi(waluau_ast::string_match_result_types(
                    &waluau_ast::expr_pattern_captures(&args[1]),
                ))))
            }
            STRING_GSUB => {
                if args.len() < 3 || args.len() > 4 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_GSUB} expects 3 or 4 arguments, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_GSUB,
                    "source",
                    &args[0],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                Some(Ok(Type::Multi(vec![Type::String, i32_ty])))
            }
            STRING_GMATCH => Some(Err(Diagnostic::new(format!(
                "{STRING_GMATCH} is only supported as a for-in iterator"
            )))),
            STRING_LEN | STRING_UPPER | STRING_LOWER | STRING_REVERSE => {
                if args.len() != 1 {
                    return Some(Err(Diagnostic::new(format!(
                        "{name} expects 1 argument, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) =
                    self.require_inferred_arg_type(name, "value", &args[0], Type::String, types)
                {
                    return Some(Err(error));
                }
                Some(Ok(if name == STRING_LEN {
                    i32_ty
                } else {
                    Type::String
                }))
            }
            STRING_SUB => {
                if args.len() < 2 || args.len() > 3 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_SUB} expects 2 or 3 arguments, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_SUB,
                    "value",
                    &args[0],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                for (index, label) in [(1, "first"), (2, "last")] {
                    if let Some(arg) = args.get(index) {
                        if let Err(error) = self.require_inferred_arg_type(
                            STRING_SUB,
                            label,
                            arg,
                            i32_ty.clone(),
                            types,
                        ) {
                            return Some(Err(error));
                        }
                    }
                }
                Some(Ok(Type::String))
            }
            STRING_REP => {
                if args.len() < 2 || args.len() > 3 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_REP} expects 2 or 3 arguments, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_REP,
                    "value",
                    &args[0],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_REP,
                    "count",
                    &args[1],
                    i32_ty.clone(),
                    types,
                ) {
                    return Some(Err(error));
                }
                if let Some(separator) = args.get(2) {
                    if let Err(error) = self.require_inferred_arg_type(
                        STRING_REP,
                        "separator",
                        separator,
                        Type::String,
                        types,
                    ) {
                        return Some(Err(error));
                    }
                }
                Some(Ok(Type::String))
            }
            STRING_SPLIT => {
                if args.is_empty() || args.len() > 2 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_SPLIT} expects 1 or 2 arguments, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_SPLIT,
                    "source",
                    &args[0],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                if let Some(separator) = args.get(1) {
                    if let Err(error) = self.require_inferred_arg_type(
                        STRING_SPLIT,
                        "separator",
                        separator,
                        Type::String,
                        types,
                    ) {
                        return Some(Err(error));
                    }
                }
                Some(Ok(Type::Array(Arc::new(Type::String))))
            }
            STRING_BYTE => {
                if args.is_empty() || args.len() > 3 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_BYTE} expects 1 to 3 arguments, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_BYTE,
                    "value",
                    &args[0],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                if let Some(index) = args.get(1) {
                    if let Err(error) = self.require_inferred_arg_type(
                        STRING_BYTE,
                        "index",
                        index,
                        i32_ty.clone(),
                        types,
                    ) {
                        return Some(Err(error));
                    }
                }
                if let Some(last) = args.get(2) {
                    if let Err(error) = self.require_inferred_arg_type(
                        STRING_BYTE,
                        "last",
                        last,
                        i32_ty.clone(),
                        types,
                    ) {
                        return Some(Err(error));
                    }
                    let count = match string_byte_static_count(args, expected.as_ref()) {
                        Ok(count) => count,
                        Err(error) => return Some(Err(error)),
                    };
                    return Some(Ok(Type::Multi(vec![i32_ty; count])));
                }
                if string_byte_scalar_known_in_range(args)
                    || string_byte_requires_numeric_value(expected.as_ref())
                {
                    Some(Ok(i32_ty))
                } else {
                    Some(Ok(Type::Nullable(Arc::new(i32_ty))))
                }
            }
            STRING_CHAR => {
                let mut arg_types = Vec::new();
                for arg in args {
                    let arg_ty = match self.infer_expr_type(arg, types, Some(i32_ty.clone())) {
                        Ok(ty) => ty,
                        Err(_) => match self.infer_expr_type(arg, types, None) {
                            Ok(ty) => ty,
                            Err(error) => return Some(Err(error)),
                        },
                    };
                    match arg_ty {
                        Type::Multi(types) => arg_types.extend(types),
                        ty => arg_types.push(ty),
                    }
                }
                if arg_types.len() > 16 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_CHAR} expects at most 16 statically expanded arguments, got {}",
                        arg_types.len()
                    ))));
                }
                for arg_ty in arg_types {
                    if arg_ty != i32_ty {
                        return Some(Err(Diagnostic::new(format!(
                            "{STRING_CHAR} expects i32 character codes, got {arg_ty}"
                        ))));
                    }
                }
                Some(Ok(Type::String))
            }
            STRING_FORMAT => {
                if args.is_empty() || args.len() > 101 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_FORMAT} expects 1 to 101 arguments, got {}",
                        args.len()
                    ))));
                }
                if let Err(error) = self.require_inferred_arg_type(
                    STRING_FORMAT,
                    "format",
                    &args[0],
                    Type::String,
                    types,
                ) {
                    return Some(Err(error));
                }
                let mut format_arg_types = Vec::new();
                for arg in args.iter().skip(1) {
                    let arg_ty = match self.infer_expr_type(arg, types, None) {
                        Ok(ty) => ty,
                        Err(error) => return Some(Err(error)),
                    };
                    match arg_ty {
                        Type::Multi(types) => format_arg_types.extend(types),
                        ty => format_arg_types.push(ty),
                    }
                }
                if format_arg_types.len() > 100 {
                    return Some(Err(Diagnostic::new(format!(
                        "{STRING_FORMAT} expects at most 100 statically expanded format arguments, got {}",
                        format_arg_types.len()
                    ))));
                }
                for arg_ty in format_arg_types {
                    if !(arg_ty.is_numeric()
                        || arg_ty == Type::Bool
                        || arg_ty == Type::String
                        || arg_ty == Type::Unknown)
                    {
                        return Some(Err(Diagnostic::new(format!(
                            "{STRING_FORMAT} expects primitive format arguments, got {arg_ty}",
                        ))));
                    }
                }
                Some(Ok(Type::String))
            }
            _ => None,
        }
    }

    fn emit_string_host_call(
        &mut self,
        host_name: &str,
        args: Vec<ValueId>,
        return_type: Type,
    ) -> Result<ValueId, Diagnostic> {
        let symbol_id = self
            .cx
            .host_import_names
            .get(host_name)
            .copied()
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "declared function '{host_name}' is missing a host import symbol"
                ))
            })?;
        Ok(self.emit(Instruction::HostCall {
            name: host_name.to_string(),
            symbol_id,
            args,
            return_type,
        }))
    }

    fn lower_nullable_i32_sentinel(&mut self, raw: ValueId, sentinel: i32) -> ValueId {
        let i32_ty = Type::Numeric(NumericType::I32);
        let nullable_i32 = Type::Nullable(Arc::new(i32_ty.clone()));
        let sentinel = self.emit_i32_const(sentinel);
        let is_nil = self.emit(Instruction::Binary {
            op: BinaryOp::Eq,
            left: raw,
            right: sentinel,
            operand_ty: i32_ty.clone(),
            result_ty: Type::Bool,
        });
        let nil_block = self.new_block();
        let value_block = self.new_block();
        let merge_block = self.new_block();
        self.set_terminator(
            self.current_block,
            Terminator::Branch {
                condition: is_nil,
                then_block: nil_block,
                else_block: value_block,
            },
        );

        self.current_block = nil_block;
        let nil = self.emit(Instruction::Null {
            ty: nullable_i32.clone(),
        });
        self.set_terminator(nil_block, Terminator::Jump(merge_block));

        self.current_block = value_block;
        let value = self.emit(Instruction::Cast {
            value: raw,
            from: i32_ty,
            to: nullable_i32,
        });
        self.set_terminator(value_block, Terminator::Jump(merge_block));

        self.current_block = merge_block;
        self.emit(Instruction::Phi(vec![(nil_block, nil), (value_block, value)]))
    }

    fn lower_expr_to_string(
        &mut self,
        expr: &Expr,
        scope: &Scope,
    ) -> Result<ValueId, Diagnostic> {
        let arg_ty = self.infer_expr_type(expr, &scope.types, None)?;
        if !(arg_ty.is_numeric()
            || arg_ty == Type::Bool
            || arg_ty == Type::String
            || arg_ty == Type::Unknown)
        {
            return Err(Diagnostic::new(format!(
                "{STRING_FORMAT} expects primitive format arguments, got {arg_ty}",
            )));
        }
        let lowered = self.lower_expr(expr, scope, Some(arg_ty.clone()))?;
        if arg_ty == Type::String {
            Ok(lowered)
        } else {
            Ok(self.emit(Instruction::ToString {
                value: lowered,
                from: arg_ty,
            }))
        }
    }

    fn require_inferred_arg_type(
        &self,
        builtin: &str,
        label: &str,
        arg: &Expr,
        expected: Type,
        types: &HashMap<SymbolId, Type>,
    ) -> Result<(), Diagnostic> {
        match self.infer_expr_type(arg, types, Some(expected.clone())) {
            Ok(actual) if actual == expected => Ok(()),
            // Multi-value results adjust to their first value in argument
            // position; accept them when that value coerces.
            Ok(actual)
                if coerce_type(actual.clone(), Some(expected.clone()))
                    .map(|ty| ty == expected)
                    .unwrap_or(false) =>
            {
                Ok(())
            }
            Ok(actual) => Err(Diagnostic::new(format!(
                "{builtin} expects {label} to be {expected}, got {actual}"
            ))),
            Err(error) => Err(error),
        }
    }
}

fn common_element_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Numeric(left), Type::Numeric(right)) => {
            left.common(right).map(Type::Numeric).ok_or_else(|| {
                inference_diagnostic(
                    "inference/conflict",
                    DiagnosticCategory::Conflict,
                    "array literal elements must share a common type",
                    "cast elements to a common numeric type or annotate the array type",
                )
            })
        }
        (left, right) if left == right => Ok(left),
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "array literal elements must share a common type",
            "cast elements to a common type or split values into separate arrays",
        )),
    }
}

fn infer_numeric_common_type(
    left: &Expr,
    right: &Expr,
    _types: &HashMap<SymbolId, Type>,
    expected: Option<NumericType>,
    infer: impl Fn(&Expr, Option<Type>) -> Result<Type, Diagnostic>,
) -> Result<Type, Diagnostic> {
    // Match HIR inference: numeric binary operands consume only the first
    // value of a multi-value result before their common type is selected.
    match (
        matches!(left, Expr::Number(..)),
        matches!(right, Expr::Number(..)),
    ) {
        (true, true) => {
            let ty = Type::Numeric(expected.unwrap_or(NumericType::F64));
            let left_ty = first_of_multi(infer(left, Some(ty.clone()))?);
            let right_ty = first_of_multi(infer(right, Some(ty))?);
            if left_ty == right_ty {
                Ok(left_ty)
            } else {
                Err(inference_diagnostic(
                    "inference/ambiguous",
                    DiagnosticCategory::Ambiguous,
                    "could not resolve operand type during IR lowering",
                    "add an explicit cast to disambiguate numeric operand types",
                ))
            }
        }
        (true, false) => {
            let right_ty = first_of_multi(infer(right, None)?);
            let left_ty = first_of_multi(infer(left, Some(right_ty.clone()))?);
            common_numeric_type(left_ty, right_ty)
        }
        (false, true) => {
            let left_ty = first_of_multi(infer(left, None)?);
            let right_ty = first_of_multi(infer(right, Some(left_ty.clone()))?);
            common_numeric_type(left_ty, right_ty)
        }
        (false, false) => {
            let left_ty = first_of_multi(infer(left, None)?);
            let right_ty = first_of_multi(infer(right, None)?);
            common_numeric_type(left_ty, right_ty)
        }
    }
}

/// True for numeric-for bounds that are untyped number literals (optionally
/// behind unary minus, e.g. the `-1` step of a countdown loop). Such bounds
/// carry no numeric type of their own and adopt the type of the loop's typed
/// bounds, mirroring how untyped literals behave in binary expressions.
/// Mirrors `waluau_hir::numeric::is_untyped_literal_bound`.
fn is_untyped_literal_bound(expr: &Expr) -> bool {
    match expr {
        Expr::Number(..) => true,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
            ..
        } => is_untyped_literal_bound(expr),
        _ => false,
    }
}

/// Infers the loop-variable type of a numeric `for` from its bound
/// expressions: typed bounds are inferred first and unified, then untyped
/// literal bounds adopt that type (defaulting to f64 when every bound is an
/// untyped literal). Mirrors `waluau_hir::numeric::infer_numeric_for_loop_type`.
fn infer_numeric_for_loop_type(
    bounds: &[&Expr],
    mut infer: impl FnMut(&Expr, Option<Type>) -> Result<Type, Diagnostic>,
) -> Result<Type, Diagnostic> {
    let mut typed: Option<Type> = None;
    for bound in bounds.iter().filter(|b| !is_untyped_literal_bound(b)) {
        let ty = infer(bound, None)?;
        typed = Some(match typed {
            None => ty,
            Some(prev) => common_numeric_type(prev, ty)?,
        });
    }
    let mut loop_ty = typed.unwrap_or(Type::Numeric(NumericType::F64));
    if !matches!(loop_ty, Type::Numeric(_)) {
        return Err(Diagnostic::new("numeric for-loop bounds must be numeric"));
    }
    for bound in bounds.iter().filter(|b| is_untyped_literal_bound(b)) {
        let ty = infer(bound, Some(loop_ty.clone()))?;
        loop_ty = common_numeric_type(loop_ty, ty)?;
    }
    Ok(loop_ty)
}

fn common_numeric_type(left: Type, right: Type) -> Result<Type, Diagnostic> {
    match (left, right) {
        (Type::Unknown, Type::Unknown)
        | (Type::Unknown, Type::Numeric(_))
        | (Type::Numeric(_), Type::Unknown) => Ok(Type::number()),
        (Type::Numeric(left), Type::Numeric(right)) => {
            left.common(right).map(Type::Numeric).ok_or_else(|| {
                inference_diagnostic(
                    "inference/ambiguous",
                    DiagnosticCategory::Ambiguous,
                    "could not resolve operand type during IR lowering",
                    "add an explicit cast to disambiguate numeric operand types",
                )
            })
        }
        _ => Err(inference_diagnostic(
            "inference/conflict",
            DiagnosticCategory::Conflict,
            "could not resolve operand type during IR lowering",
            "change one operand type or cast explicitly",
        )),
    }
}

/// `table.concat` joins string elements, and — like Luau — numeric elements,
/// which it stringifies with the same formatting `tostring` uses. Returns the
/// accepted element type so the join loop can read and convert with it.
fn table_concat_element_type(list_ty: &Type) -> Option<Type> {
    let element = list_ty.element_type()?;
    (element == Type::String || element.is_numeric()).then_some(element)
}

/// Types `tostring` accepts: primitives stringify by value, `nil` folds to
/// "nil", reference types (functions, tables, threads) format their identity,
/// and nullable/unknown values dispatch at runtime.
fn tostring_supported_type(ty: &Type) -> bool {
    ty.is_numeric()
        || matches!(
            ty,
            Type::Bool
                | Type::String
                | Type::Unknown
                | Type::Nil
                | Type::Function { .. }
                | Type::Array(_)
                | Type::Record(_)
                | Type::TaggedVariant(_)
                | Type::TaggedUnion(_)
                | Type::Thread
                | Type::Nullable(_)
        )
}

fn string_concat_operand_type(ty: &Type) -> bool {
    ty == &Type::String
        || ty == &Type::Unknown
        || ty.is_numeric()
        || matches!(ty, Type::Nullable(inner) if inner.as_ref() == &Type::String)
}

/// Collapses a multi-value type to the single value Lua adjusts it to.
///
/// A call in single-value position yields its first result, or `nil` when it
/// returns nothing.
fn adjust_to_single_value(ty: Type) -> Type {
    match ty {
        Type::Multi(mut parts) => {
            if parts.is_empty() {
                Type::Nil
            } else {
                adjust_to_single_value(parts.remove(0))
            }
        }
        other => other,
    }
}

/// The Lua type name `type()`/`typeof()` reports for a statically known type.
///
/// Returns `None` when the name cannot be determined statically. Callers must
/// report a diagnostic rather than emit a placeholder: real Luau `type()` only
/// ever returns one of the names below, so any other string would be a value no
/// Luau program can produce.
fn lua_type_name(ty: &Type) -> Option<&'static str> {
    Some(match ty {
        Type::Nil => "nil",
        Type::Unit => "nil",
        Type::Bool => "boolean",
        Type::Numeric(_) => "number",
        Type::String | Type::Bytes | Type::StringLiteralUnion(_) => "string",
        Type::Array(_)
        | Type::Variadic(_)
        | Type::Record(_)
        | Type::TaggedVariant(_)
        | Type::TaggedUnion(_) => "table",
        Type::Function { .. } => "function",
        Type::Thread => "thread",
        // Matches Luau, where typeof(buffer.create(n)) == "buffer".
        Type::TypedArray(_) | Type::Buffer => "buffer",
        Type::Extern | Type::ExternSubtype(_) => "userdata",
        Type::Nullable(_) => "nil",
        // A nominal alias (`type Point = { x: number }`) is transparent to
        // `type()`, which reports the underlying representation's name.
        Type::Opaque { ty, .. } => return lua_type_name(ty),
        // Callers adjust multi-value results before classifying them.
        Type::Multi(_) => return None,
        // Dispatched at runtime by the caller, never named statically.
        Type::Unknown => return None,
        // An unresolved alias has no known representation, and a generic type
        // parameter is only known once monomorphized; neither has a static name.
        Type::Named { .. } | Type::TypeParam(_) => return None,
    })
}

fn required_param_count(params: &[Type]) -> usize {
    params
        .iter()
        .rposition(|param| !param.accepts_nil())
        .map_or(0, |index| index + 1)
}

fn call_arity_matches(params: &[Type], actual: usize) -> bool {
    actual >= required_param_count(params)
}

/// Whether a multi-value result adjusts to a narrower multi-value context by
/// dropping its trailing values. Packs (`Variadic`) forward a runtime-sized
/// pack and keep their own expansion rules, so they never truncate here.
/// Mirrors `multi_truncates_to` in waluau-hir.
fn multi_truncates_to(actual: &Type, expected_parts: &[Type]) -> bool {
    let Type::Multi(actual_parts) = actual else {
        return false;
    };
    actual_parts.len() > expected_parts.len()
        && !actual_parts
            .iter()
            .any(|ty| matches!(ty, Type::Variadic(_)))
        && !expected_parts
            .iter()
            .any(|ty| matches!(ty, Type::Variadic(_)))
}

fn type_record_fields(ty: &Type) -> Option<&BTreeMap<String, Type>> {
    match ty {
        Type::Record(fields) => Some(fields),
        Type::Opaque { ty, .. } => type_record_fields(ty),
        Type::Nullable(ty) => type_record_fields(ty),
        _ => None,
    }
}

fn coerce_type(actual: Type, expected: Option<Type>) -> Result<Type, Diagnostic> {
    // A tagged union nested inside an aggregate is named by its source type in
    // some annotations and by the canonical `{ tag, value }` record that
    // represents it in others — `{Goods}` against `{{tag, value}}`, say. Those
    // describe the same value, and `to_runtime_type` rewrites nothing else, so
    // agreeing under it means the annotations differ only in that naming. A bare
    // tagged type is left to the tag-by-tag arms below.
    if let Some(expected_ty) = &expected {
        if !matches!(actual, Type::TaggedUnion(_) | Type::TaggedVariant(_))
            && !matches!(expected_ty, Type::TaggedUnion(_) | Type::TaggedVariant(_))
            && actual != *expected_ty
            && to_runtime_type(&actual) == to_runtime_type(expected_ty)
        {
            return Ok(expected.expect("checked in the guard above"));
        }
    }
    match expected {
        None => Ok(actual),
        Some(expected) if actual == expected => Ok(expected),
        // A multi-value result in a single-value context collapses to its
        // first value, following Lua's adjustment rules.
        Some(expected)
            if matches!(actual, Type::Multi(_)) && !matches!(expected, Type::Multi(_)) =>
        {
            let Type::Multi(mut parts) = actual else {
                unreachable!()
            };
            if parts.is_empty() {
                return Err(Diagnostic::new(
                    "cannot use an empty multi-value result as a value",
                ));
            }
            coerce_type(parts.remove(0), Some(expected))
        }
        // The same adjustment at tuple width: a multi-value expression wider
        // than its context drops the trailing values.
        Some(Type::Multi(expected_parts)) if multi_truncates_to(&actual, &expected_parts) => {
            let Type::Multi(actual_parts) = actual else {
                unreachable!()
            };
            actual_parts
                .into_iter()
                .zip(expected_parts)
                .map(|(actual_ty, expected_ty)| coerce_type(actual_ty, Some(expected_ty)))
                .collect::<Result<Vec<_>, _>>()
                .map(Type::Multi)
        }
        // Nullable values are truthy exactly when non-nil, so they coerce to
        // bool in condition positions.
        Some(Type::Bool) if matches!(actual, Type::Nullable(_)) => Ok(Type::Bool),
        // Any value implicitly boxes into `unknown` (anyref). Symmetrically, an
        // `unknown` value (e.g. an unannotated Lua parameter) implicitly
        // unboxes to any concrete type with a runtime-checked cast, mirroring
        // Lua's dynamic typing.
        Some(Type::Unknown) => Ok(Type::Unknown),
        Some(expected) if actual == Type::Unknown && expected != Type::Unit => Ok(expected),
        Some(Type::Nullable(expected_inner)) => match actual {
            Type::Nil => Ok(Type::Nullable(expected_inner)),
            Type::Nullable(actual_inner) if actual_inner == expected_inner => {
                Ok(Type::Nullable(expected_inner))
            }
            other => coerce_type(other.clone(), Some((*expected_inner).clone()))
                .map(|_| Type::Nullable(expected_inner.clone()))
                .map_err(|_| {
                    Diagnostic::new(format!(
                        "cannot implicitly convert {other} to {}?",
                        expected_inner
                    ))
                }),
        },
        // A constructor produces the single variant it names; that variant widens
        // to any union declaring the same tag. Both are the canonical
        // `{ tag, value }` record at the IR level, so this is a type-level
        // widening with no lowering behind it — which is also why a value that has
        // already been reduced to that record still coerces here.
        Some(Type::TaggedUnion(expected_variants)) => match &actual {
            Type::TaggedVariant(actual_variant)
                if expected_variants
                    .iter()
                    .any(|variant| variant.tag == actual_variant.tag) =>
            {
                Ok(Type::TaggedUnion(expected_variants))
            }
            Type::TaggedUnion(actual_variants)
                if actual_variants.iter().all(|actual_variant| {
                    expected_variants
                        .iter()
                        .any(|variant| variant.tag == actual_variant.tag)
                }) =>
            {
                Ok(Type::TaggedUnion(expected_variants))
            }
            _ if actual == Type::canonical_tagged_union_record() => {
                Ok(Type::TaggedUnion(expected_variants))
            }
            _ => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {}",
                Type::TaggedUnion(expected_variants)
            ))),
        },
        Some(Type::TaggedVariant(expected_variant)) => match &actual {
            Type::TaggedVariant(actual_variant) if actual_variant.tag == expected_variant.tag => {
                Ok(Type::TaggedVariant(expected_variant))
            }
            _ if actual == Type::canonical_tagged_union_record() => {
                Ok(Type::TaggedVariant(expected_variant))
            }
            _ => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {}({})",
                expected_variant.tag, expected_variant.payload
            ))),
        },
        // Records coerce field-by-field so a field value can box into an `unknown`
        // field. Lowering then targets the expected field types and inserts boxes.
        Some(Type::Record(expected_fields)) => {
            // A tagged union is that record at the IR level, so a variant value
            // reaching a record-typed position is already the shape it needs.
            if matches!(actual, Type::TaggedUnion(_) | Type::TaggedVariant(_))
                && Type::Record(expected_fields.clone()) == Type::canonical_tagged_union_record()
            {
                return Ok(Type::Record(expected_fields));
            }
            let Type::Record(actual_fields) = &actual else {
                return Err(Diagnostic::new(format!(
                    "cannot implicitly convert {actual} to {}",
                    Type::Record(expected_fields)
                )));
            };
            for (name, expected_ty) in expected_fields.iter() {
                let Some(actual_ty) = actual_fields.get(name) else {
                    if expected_ty.accepts_nil() {
                        continue;
                    }
                    return Err(Diagnostic::new(format!("missing record field '{name}'")));
                };
                coerce_type(actual_ty.clone(), Some(expected_ty.clone())).map_err(|_| {
                    Diagnostic::new(format!(
                        "record field '{name}' expects {expected_ty}, got {actual_ty}"
                    ))
                })?;
            }
            Ok(Type::Record(expected_fields))
        }
        Some(Type::Numeric(expected_numeric)) => match actual {
            Type::Numeric(actual_numeric)
                if actual_numeric.can_implicitly_widen_to(expected_numeric) =>
            {
                Ok(Type::Numeric(expected_numeric))
            }
            Type::Numeric(actual_numeric) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual_numeric} to {expected_numeric}",
            ))),
            Type::StringLiteralUnion(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert string literal union to {expected_numeric}",
            ))),
            Type::Bool => Err(Diagnostic::new(format!(
                "cannot implicitly convert bool to {expected_numeric}",
            ))),
            Type::Unit => Err(Diagnostic::new(format!(
                "cannot implicitly convert unit to {expected_numeric}",
            ))),
            Type::String => Err(Diagnostic::new(format!(
                "cannot implicitly convert string to {expected_numeric}",
            ))),
            Type::Bytes => Err(Diagnostic::new(format!(
                "cannot implicitly convert bytes to {expected_numeric}",
            ))),
            Type::Buffer => Err(Diagnostic::new(format!(
                "cannot implicitly convert buffer to {expected_numeric}",
            ))),
            Type::Extern | Type::ExternSubtype(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert extern to {expected_numeric}",
            ))),
            Type::Nil => Err(Diagnostic::new(format!(
                "cannot implicitly convert nil to {expected_numeric}",
            ))),
            Type::Nullable(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert nullable value to {expected_numeric}",
            ))),
            Type::Named { name, .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert {name} to {expected_numeric}",
            ))),
            Type::Opaque { name, .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert {name} to {expected_numeric}",
            ))),
            Type::Array(_) | Type::Variadic(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert array to {expected_numeric}",
            ))),
            Type::TypedArray(kind) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {} to {expected_numeric}",
                kind.type_name(),
            ))),
            Type::Multi(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert multiple values to {expected_numeric}",
            ))),
            Type::Function { .. } => Err(Diagnostic::new(format!(
                "cannot implicitly convert function to {expected_numeric}",
            ))),
            Type::Record(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert namespace to {expected_numeric}",
            ))),
            Type::TypeParam(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert generic type parameter to {expected_numeric}",
            ))),
            Type::Thread => Err(Diagnostic::new(format!(
                "cannot implicitly convert thread to {expected_numeric}",
            ))),
            Type::Unknown => Err(Diagnostic::new(format!(
                "cannot implicitly convert unknown to {expected_numeric}; use an explicit cast",
            ))),
            Type::TaggedVariant(_) | Type::TaggedUnion(_) => Err(Diagnostic::new(format!(
                "cannot implicitly convert {actual} to {expected_numeric}",
            ))),
        },
        Some(Type::Bool) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to bool",
        ))),
        Some(Type::Unit) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to unit",
        ))),
        Some(expected) => Err(Diagnostic::new(format!(
            "cannot implicitly convert {actual} to {expected}",
        ))),
    }
}

pub(crate) fn require_numeric_cast(actual: Type, target: Type) -> Result<(), Diagnostic> {
    match (&actual, &target) {
        (Type::Opaque { ty, .. }, target) if ty.as_ref() == target => Ok(()),
        (actual, Type::Opaque { ty, .. }) if actual == ty.as_ref() => Ok(()),
        // Boxing into / unboxing out of `unknown` (anyref) is an explicit cast.
        (_, Type::Unknown) | (Type::Unknown, _) => Ok(()),
        _ => match (actual, target) {
        (Type::Numeric(_), Type::Numeric(_)) => Ok(()),
        _ => Err(Diagnostic::new(
            "casts require numeric source and destination types",
        )),
        },
    }
}

fn block_mut(function: &mut Function, block: BlockId) -> &mut BasicBlock {
    function
        .blocks
        .get_mut(&block)
        .expect("block must exist when mutating")
}

/// Keep phi inputs in the same canonical order as the verifier's CFG
/// predecessor lists. Lowering often visits source branches in then/else
/// order, but a nested CFG in the then branch can make its exit block newer
/// than the else exit. Block labels make those edges unambiguous; sorting at
/// the emission boundary ensures every lowering path represents them in the
/// verifier's deterministic block-id order.
fn sort_phi_incoming(instruction: &mut Instruction) {
    if let Instruction::Phi(incoming) = instruction {
        incoming.sort_by_key(|(predecessor, _)| *predecessor);
    }
}

/// A cast names the type it converts from and the type it converts to, and both
/// annotations describe runtime values, so both have to be written in the
/// runtime representation. Source-level annotations reach the two sides through
/// different routes — one from an inferred expression type, the other from a
/// declared one — and a tagged union nested in an aggregate is spelled by its
/// source type on one route and by the canonical `{ tag, value }` record on the
/// other. Normalizing at the emission boundary, the way phi inputs are sorted
/// there, means no lowering path can emit the two halves in different spellings
/// and leave the verifier to read a conversion into what converts nothing.
fn canonicalize_cast_types(instruction: &mut Instruction) {
    match instruction {
        Instruction::Cast { from, to, .. } => {
            *from = to_runtime_type(from);
            *to = to_runtime_type(to);
        }
        Instruction::MultiGet { ty, .. } => {
            *ty = to_runtime_type(ty);
        }
        _ => {}
    }
}

fn add_phi_incoming(
    function: &mut Function,
    block: BlockId,
    phi: ValueId,
    incoming: (BlockId, ValueId),
) {
    let bb = block_mut(function, block);
    let (_, instruction) = bb
        .instructions
        .iter_mut()
        .find(|(value, _)| *value == phi)
        .expect("phi value must exist in header block");
    if let Instruction::Phi(values) = instruction {
        // The verifier expects phi incomings in predecessor order, which is
        // ascending block id (function.blocks is a BTreeMap). Break edges can
        // be registered after a later-created normal exit edge, so keep the
        // list sorted on insert instead of relying on registration order.
        let position = values
            .iter()
            .position(|(pred, _)| *pred > incoming.0)
            .unwrap_or(values.len());
        values.insert(position, incoming);
    }
}
