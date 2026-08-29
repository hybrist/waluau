//! Client-side language server for the playground editor.
//!
//! [`WaluauLanguageServer`] wraps the transport-agnostic [`waluau_lsp`] core
//! with an in-memory analysis backend. The browser replaces the full virtual
//! project in one batch, then uses JSON-RPC for position-based language
//! features. Compilation and diagnostics share one linked, typed program so
//! project updates do not repeat front-end analysis.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use waluau_diagnostics::Diagnostic;
use waluau_lsp::{AnalysisBackend, BackendAnalysis, LspServer};
use wasm_bindgen::prelude::*;

use crate::{CompileResult, compile_typed_program, link};

#[derive(Serialize)]
struct ProjectPosition {
    line: u32,
    character: u32,
}

#[derive(Serialize)]
struct ProjectRange {
    start: ProjectPosition,
    end: ProjectPosition,
}

#[derive(Serialize)]
struct ProjectDiagnostic {
    range: ProjectRange,
    severity: u8,
    source: &'static str,
    message: String,
    /// LSP/Monaco diagnostic tags (1 = Unnecessary, rendered faded).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectAnalysis {
    output: Option<CompileResult>,
    error_msg: String,
    diagnostics: BTreeMap<String, Vec<ProjectDiagnostic>>,
}

#[derive(Default)]
struct PlaygroundBackend {
    /// Virtual file map fed by LSP document overlays, keyed by absolute
    /// playground paths such as `/main.walu`.
    files: HashMap<String, String>,
}

impl AnalysisBackend for PlaygroundBackend {
    fn set_overlay(&mut self, path: &Path, content: &str) {
        self.files
            .insert(path.to_string_lossy().into_owned(), content.to_string());
    }

    fn remove_overlay(&mut self, path: &Path) {
        self.files.remove(path.to_string_lossy().as_ref());
    }

    fn analyze_root(&mut self, root: &Path) -> BackendAnalysis {
        let root = root.to_string_lossy();
        let (program, parse_diagnostics) = match link::link_programs_collect(&self.files, &root) {
            Ok(outcome) => outcome,
            Err(message) => {
                return BackendAnalysis {
                    diagnostics: vec![Diagnostic::new(message)],
                };
            }
        };
        if !parse_diagnostics.is_empty() {
            return BackendAnalysis {
                diagnostics: parse_diagnostics,
            };
        }
        let diagnostics = match waluau_hir::type_check_and_infer_collect(&program) {
            Ok(_) => Vec::new(),
            Err(errors) => errors
                .into_iter()
                .map(|error| resolve_source(error, &program))
                .collect(),
        };
        BackendAnalysis { diagnostics }
    }
}

fn resolve_source(error: Diagnostic, program: &waluau_ast::Program) -> Diagnostic {
    let file_path = error
        .file_path()
        .unwrap_or(&program.entry_file_path)
        .to_string();
    match program.sources.get(&file_path) {
        Some(source) => error.with_source(file_path, source),
        None => error.with_file_path_if_missing(file_path),
    }
}

#[wasm_bindgen]
pub struct WaluauLanguageServer {
    inner: LspServer<PlaygroundBackend>,
}

impl Default for WaluauLanguageServer {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WaluauLanguageServer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WaluauLanguageServer {
        WaluauLanguageServer {
            inner: LspServer::with_backend(PlaygroundBackend::default()),
        }
    }

    /// Handle one JSON-RPC message string; returns the outgoing message
    /// strings (responses and publishDiagnostics notifications).
    #[wasm_bindgen(js_name = handleMessage)]
    pub fn handle_message(&mut self, message: &str) -> Vec<String> {
        self.inner.handle_message(message)
    }

    /// Replace the complete browser project and analyze it once.
    ///
    /// This is deliberately a combined operation: linking and type inference
    /// produce both structured editor diagnostics and the typed program used
    /// for code generation. Successful projects therefore do not repeat the
    /// compiler's most expensive front-end work in a separate LSP pass.
    #[wasm_bindgen(js_name = analyzeProject)]
    pub fn analyze_project(
        &mut self,
        files: JsValue,
        entry_path: &str,
    ) -> Result<JsValue, JsValue> {
        let files: HashMap<String, String> = serde_wasm_bindgen::from_value(files)
            .map_err(|error| JsValue::from_str(&format!("failed to parse files map: {error}")))?;
        self.inner.replace_documents(
            files
                .iter()
                .map(|(path, source)| (PathBuf::from(path), source.clone()))
                .collect(),
        );
        serde_wasm_bindgen::to_value(&analyze_project_sources(&files, entry_path))
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Synchronize the live editor buffer used by a position request without
    /// starting a whole-project diagnostics pass.
    #[wasm_bindgen(js_name = updateDocument)]
    pub fn update_document(&mut self, path: &str, text: String) {
        self.inner.update_document(PathBuf::from(path), text);
    }
}

fn analyze_project_sources(files: &HashMap<String, String>, entry_path: &str) -> ProjectAnalysis {
    let entry_path = normalized_project_path(entry_path);
    let mut roots = vec![entry_path.clone()];
    let mut remaining = files
        .keys()
        .map(|path| normalized_project_path(path))
        .filter(|path| path != &entry_path)
        .collect::<Vec<_>>();
    remaining.sort();
    remaining.dedup();
    roots.extend(remaining);

    let mut covered_files = HashSet::new();
    let mut sources = BTreeMap::new();
    let mut all_diagnostics = Vec::new();
    let mut output = None;
    let mut error_msg = String::new();

    for (index, root) in roots.into_iter().enumerate() {
        let is_entry = index == 0;
        if !is_entry && covered_files.contains(&root) {
            continue;
        }
        let (program, parse_diagnostics) = match link::link_programs_collect(files, &root) {
            Ok(outcome) => outcome,
            Err(message) => {
                let diagnostic = Diagnostic::new(message).with_file_path(&root);
                if is_entry {
                    error_msg = diagnostic.render_for_playground();
                }
                covered_files.insert(root);
                all_diagnostics.push(diagnostic);
                continue;
            }
        };
        covered_files.extend(
            program
                .sources
                .keys()
                .map(|path| normalized_project_path(path)),
        );
        sources.extend(program.sources.clone());

        if !parse_diagnostics.is_empty() {
            let diagnostics = parse_diagnostics
                .into_iter()
                .map(|error| resolve_source(error, &program))
                .collect::<Vec<_>>();
            if is_entry {
                error_msg = diagnostics
                    .first()
                    .map(Diagnostic::render_for_playground)
                    .unwrap_or_default();
            }
            all_diagnostics.extend(diagnostics);
            continue;
        }

        match waluau_hir::type_check_and_infer_collect(&program) {
            Ok(typed_program) if is_entry => match compile_typed_program(&typed_program) {
                Ok(compiled) => output = Some(compiled),
                Err(error) => error_msg = error,
            },
            Ok(_) => {}
            Err(errors) => {
                let diagnostics = errors
                    .into_iter()
                    .map(|error| resolve_source(error, &program))
                    .collect::<Vec<_>>();
                if is_entry {
                    error_msg = diagnostics
                        .first()
                        .map(Diagnostic::render_for_playground)
                        .unwrap_or_default();
                }
                all_diagnostics.extend(diagnostics);
            }
        }
    }

    // Single-file lint warnings for every project file. These never gate
    // compilation or the entry error message; they only add markers.
    for (path, source) in files {
        let path = normalized_project_path(path);
        // Files a failed link never registered still need their text for
        // span-to-position conversion below.
        sources
            .entry(path.clone())
            .or_insert_with(|| source.clone());
        for diagnostic in waluau_lsp::unused_definitions(source, Path::new(&path)) {
            all_diagnostics.push(diagnostic.with_source(&path, source));
        }
    }

    ProjectAnalysis {
        output,
        error_msg,
        diagnostics: diagnostics_by_uri(all_diagnostics, &sources, &entry_path),
    }
}

fn normalized_project_path(path: &str) -> String {
    let mut normalized = link::clean_path(path);
    if !normalized.ends_with(".walu") && Path::new(&normalized).extension().is_none() {
        normalized.push_str(".walu");
    }
    normalized
}

fn diagnostics_by_uri(
    diagnostics: Vec<Diagnostic>,
    sources: &BTreeMap<String, String>,
    entry_path: &str,
) -> BTreeMap<String, Vec<ProjectDiagnostic>> {
    let mut by_uri = BTreeMap::<String, Vec<ProjectDiagnostic>>::new();
    for diagnostic in diagnostics {
        let file = diagnostic.file_path().unwrap_or(entry_path);
        let source = sources.get(file);
        let (start, end) = diagnostic
            .span()
            .and_then(|span| {
                let source = source?;
                Some((
                    source_position(source, span.start as usize),
                    source_position(source, span.end as usize),
                ))
            })
            .unwrap_or((
                ProjectPosition {
                    line: 0,
                    character: 0,
                },
                ProjectPosition {
                    line: 0,
                    character: 0,
                },
            ));
        by_uri
            .entry(format!("file://{file}"))
            .or_default()
            .push(ProjectDiagnostic {
                range: ProjectRange { start, end },
                severity: match diagnostic.severity() {
                    waluau_diagnostics::Severity::Error => 1,
                    waluau_diagnostics::Severity::Warning => 2,
                },
                source: "waluau",
                message: diagnostic.to_string(),
                tags: diagnostic
                    .tag()
                    .map(|tag| match tag {
                        waluau_diagnostics::DiagnosticTag::Unnecessary => 1,
                    })
                    .into_iter()
                    .collect(),
            });
    }
    by_uri
}

fn source_position(source: &str, byte_offset: usize) -> ProjectPosition {
    let clamped = byte_offset.min(source.len());
    let mut line = 0u32;
    let mut character = 0u32;
    for (index, ch) in source.char_indices() {
        if index >= clamped {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }
    ProjectPosition { line, character }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{Value, json};

    use super::{WaluauLanguageServer, analyze_project_sources};

    fn send(server: &mut WaluauLanguageServer, message: Value) -> Vec<Value> {
        server
            .handle_message(&message.to_string())
            .into_iter()
            .map(|outgoing| serde_json::from_str(&outgoing).expect("outgoing message is JSON"))
            .collect()
    }

    fn open(server: &mut WaluauLanguageServer, uri: &str, text: &str) -> Vec<Value> {
        send(
            server,
            json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": uri, "languageId": "waluau", "version": 1, "text": text,
                }},
            }),
        )
    }

    fn diagnostics_for<'a>(messages: &'a [Value], uri: &str) -> Option<&'a Vec<Value>> {
        messages.iter().find_map(|message| {
            (message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri)
                .then(|| message["params"]["diagnostics"].as_array())
                .flatten()
        })
    }

    #[test]
    fn project_analysis_reports_unused_bindings_as_faded_warnings() {
        let analysis = analyze_project_sources(
            &HashMap::from([(
                "/main.walu".to_string(),
                "local unused = 1\nfunction answer(): i32\n    return 42\nend\n".to_string(),
            )]),
            "/main.walu",
        );

        assert!(
            analysis.output.is_some(),
            "warnings must not block compilation"
        );
        assert!(analysis.error_msg.is_empty());
        let diagnostics = analysis
            .diagnostics
            .get("file:///main.walu")
            .expect("unused-binding warning");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, 2);
        assert_eq!(diagnostics[0].tags, vec![1]);
        assert_eq!(diagnostics[0].message, "unused variable `unused`");
        assert_eq!(diagnostics[0].range.start.line, 0);
        assert_eq!(diagnostics[0].range.start.character, 6);
        assert_eq!(diagnostics[0].range.end.character, 12);
    }

    #[test]
    fn combined_project_analysis_compiles_without_a_second_diagnostics_pass() {
        let analysis = analyze_project_sources(
            &HashMap::from([(
                "/main.walu".to_string(),
                "function answer(): i32\n    return 42\nend\n".to_string(),
            )]),
            "/main.walu",
        );

        assert!(analysis.output.is_some());
        assert!(analysis.error_msg.is_empty());
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn combined_project_analysis_returns_all_structured_type_errors() {
        let analysis = analyze_project_sources(
            &HashMap::from([(
                "/main.walu".to_string(),
                "function first(x: i32): bool\n    return x\nend\nfunction second(x: i32): i32\n    if x then\n        return x\n    end\n    return x\nend\n".to_string(),
            )]),
            "/main.walu",
        );

        assert!(analysis.output.is_none());
        assert!(!analysis.error_msg.is_empty());
        let diagnostics = analysis
            .diagnostics
            .get("file:///main.walu")
            .expect("entry diagnostics");
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].source, "waluau");
    }

    #[test]
    fn combined_project_analysis_checks_only_uncovered_extra_roots() {
        let analysis = analyze_project_sources(
            &HashMap::from([
                (
                    "/main.walu".to_string(),
                    "local imported_answer = require(\"./lib\")\nfunction answer(): i32\n    return imported_answer()\nend\n"
                        .to_string(),
                ),
                (
                    "/lib.walu".to_string(),
                    "function answer(): i32\n    return 42\nend\nreturn answer\n".to_string(),
                ),
                (
                    "/scratch.walu".to_string(),
                    "function broken(value: i32): bool\n    return value\nend\n".to_string(),
                ),
            ]),
            "/main.walu",
        );

        assert!(
            analysis.output.is_some(),
            "entry graph should still compile"
        );
        assert!(analysis.error_msg.is_empty());
        let diagnostics = analysis
            .diagnostics
            .get("file:///scratch.walu")
            .expect("unreferenced file diagnostics");
        assert_eq!(diagnostics.len(), 1);
        assert!(!analysis.diagnostics.contains_key("file:///lib.walu"));
    }

    #[test]
    fn publishes_multi_error_diagnostics_for_virtual_files() {
        let mut server = WaluauLanguageServer::new();
        let messages = open(
            &mut server,
            "file:///main.walu",
            "function first(x: i32): bool\n    return x\nend\nfunction second(x: i32): i32\n    if x then\n        return x\n    end\n    return x\nend\n",
        );
        let diagnostics =
            diagnostics_for(&messages, "file:///main.walu").expect("diagnostics published");
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    }

    #[test]
    fn required_virtual_module_errors_publish_under_their_uri() {
        let mut server = WaluauLanguageServer::new();
        open(
            &mut server,
            "file:///lib.walu",
            "function double(x: i32): bool\n    return x * 2\nend\nreturn double\n",
        );
        let messages = open(
            &mut server,
            "file:///main.walu",
            "local double = require(\"./lib\")\nlocal a = double(2)\n",
        );
        let lib_diagnostics =
            diagnostics_for(&messages, "file:///lib.walu").expect("lib diagnostics");
        assert_eq!(lib_diagnostics.len(), 1, "{lib_diagnostics:?}");
    }

    #[test]
    fn definitions_cross_virtual_module_namespace() {
        let mut server = WaluauLanguageServer::new();
        // This is the in-memory document batch that `analyzeProject` installs
        // before the playground sends position requests through the worker.
        server.inner.replace_documents(HashMap::from([
            ("/e.walu".into(), "export enum E { A }".to_string()),
            (
                "/other.walu".into(),
                "local e_lib = require(\"./e\")\n\nlocal e = e_lib.E.A".to_string(),
            ),
        ]));

        for (id, character, expected_character) in [(1, 16, 12), (2, 18, 16)] {
            let messages = send(
                &mut server,
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "textDocument/definition",
                    "params": {
                        "textDocument": {"uri": "file:///other.walu"},
                        "position": {"line": 2, "character": character},
                    },
                }),
            );
            let location = &messages[0]["result"];
            assert_eq!(location["uri"], "file:///e.walu", "{location:?}");
            assert_eq!(location["range"]["start"]["line"], 0, "{location:?}");
            assert_eq!(
                location["range"]["start"]["character"], expected_character,
                "{location:?}"
            );
            assert_eq!(
                location["range"]["end"]["character"],
                expected_character + 1,
                "{location:?}"
            );
        }
    }
}
