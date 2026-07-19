//! Client-side language server for the playground editor.
//!
//! [`WaluauLanguageServer`] wraps the transport-agnostic [`waluau_lsp`] core
//! with an in-memory analysis backend: the playground opens each virtual file
//! (`file:///main.walu`, `file:///lib.walu`, ...) over the LSP document
//! lifecycle, and diagnostics come back as `textDocument/publishDiagnostics`
//! notifications — no server round-trip involved.

use std::collections::HashMap;
use std::path::Path;

use wasm_bindgen::prelude::*;
use waluau_diagnostics::Diagnostic;
use waluau_lsp::{AnalysisBackend, BackendAnalysis, LspServer};

use crate::link;

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
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::WaluauLanguageServer;

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
}
