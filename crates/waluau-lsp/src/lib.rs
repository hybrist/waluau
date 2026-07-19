//! Transport-agnostic LSP core for the Waluau compiler.
//!
//! [`LspServer`] consumes JSON-RPC message strings and produces response and
//! notification strings; it performs no I/O of its own, so the same core
//! drives the native stdio binary (`main.rs`) and — via the wasm bridge — a
//! client-side language server in the playground editor. Analysis runs on a
//! [`CompilerSession`]: every open document is an analysis root, so files not
//! imported from any entry point (new files, test files) get diagnostics too.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use waluau_diagnostics::{Diagnostic, Severity};
use waluau_driver::CompilerSession;

pub struct LspServer {
    session: CompilerSession,
    /// Open documents by filesystem path, holding the latest buffer text.
    open_documents: HashMap<PathBuf, String>,
    /// URIs we last published diagnostics for, so stale ones can be cleared.
    published_uris: HashSet<String>,
    shutdown_requested: bool,
    exit_requested: bool,
}

impl Default for LspServer {
    fn default() -> Self {
        Self::new()
    }
}

impl LspServer {
    pub fn new() -> Self {
        Self {
            session: CompilerSession::new(),
            open_documents: HashMap::new(),
            published_uris: HashSet::new(),
            shutdown_requested: false,
            exit_requested: false,
        }
    }

    /// True once the client sent `exit`; the transport should terminate.
    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    /// True when `exit` arrived after a clean `shutdown` handshake.
    pub fn clean_exit(&self) -> bool {
        self.shutdown_requested
    }

    /// Handle one incoming JSON-RPC message; returns any outgoing messages
    /// (the response, if the input was a request, plus notifications such as
    /// `textDocument/publishDiagnostics`).
    pub fn handle_message(&mut self, message: &str) -> Vec<String> {
        let parsed: Value = match serde_json::from_str(message) {
            Ok(value) => value,
            Err(error) => {
                return vec![
                    error_response(Value::Null, -32700, &format!("parse error: {error}")),
                ];
            }
        };
        let id = parsed.get("id").cloned();
        let method = parsed
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = parsed.get("params").cloned().unwrap_or(Value::Null);

        match (method.as_str(), id) {
            ("initialize", Some(id)) => vec![response(
                id,
                json!({
                    "capabilities": {
                        // 1 = full-document sync: every change carries the
                        // complete buffer, matching the session overlay model.
                        "textDocumentSync": 1,
                    },
                    "serverInfo": {"name": "waluau-lsp", "version": env!("CARGO_PKG_VERSION")},
                }),
            )],
            ("initialized", _) => Vec::new(),
            ("shutdown", Some(id)) => {
                self.shutdown_requested = true;
                vec![response(id, Value::Null)]
            }
            ("exit", _) => {
                self.exit_requested = true;
                Vec::new()
            }
            ("textDocument/didOpen", _) => {
                let Some((path, _uri)) = document_path(&params) else {
                    return Vec::new();
                };
                let text = params["textDocument"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                self.upsert_document(path, text);
                self.publish_all_diagnostics()
            }
            ("textDocument/didChange", _) => {
                let Some((path, _uri)) = document_path(&params) else {
                    return Vec::new();
                };
                // Full sync: the last content change carries the whole buffer.
                let Some(text) = params["contentChanges"]
                    .as_array()
                    .and_then(|changes| changes.last())
                    .and_then(|change| change["text"].as_str())
                    .map(str::to_string)
                else {
                    return Vec::new();
                };
                self.upsert_document(path, text);
                self.publish_all_diagnostics()
            }
            ("textDocument/didClose", _) => {
                let Some((path, _uri)) = document_path(&params) else {
                    return Vec::new();
                };
                self.open_documents.remove(&path);
                self.session.remove_overlay(&path);
                self.publish_all_diagnostics()
            }
            ("textDocument/didSave", _) => Vec::new(),
            // Unknown request (has an id): report method-not-found.
            (_, Some(id)) => vec![error_response(
                id,
                -32601,
                &format!("method not found: {method}"),
            )],
            // Unknown notification: ignore.
            (_, None) => Vec::new(),
        }
    }

    fn upsert_document(&mut self, path: PathBuf, text: String) {
        // The linker resolves modules through canonical paths; the overlay
        // must be registered under the same key the resolver produces.
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        self.session.set_overlay(canonical, text.clone());
        self.open_documents.insert(path, text);
    }

    /// Analyze every open document as its own root and publish per-file
    /// diagnostics, clearing files that no longer have any.
    fn publish_all_diagnostics(&mut self) -> Vec<String> {
        // Diagnostics can repeat when two open roots share a module with an
        // error; dedupe per file while preserving order.
        let mut per_file: BTreeMap<String, Vec<Diagnostic>> = BTreeMap::new();
        let roots: Vec<PathBuf> = self.open_documents.keys().cloned().collect();
        for root in &roots {
            let analysis = self.session.analyze_root(root);
            for diagnostic in analysis.diagnostics {
                let file = diagnostic
                    .file_path()
                    .map(str::to_string)
                    .unwrap_or_else(|| root.to_string_lossy().into_owned());
                let entries = per_file.entry(file).or_default();
                if !entries.contains(&diagnostic) {
                    entries.push(diagnostic);
                }
            }
        }

        let mut messages = Vec::new();
        let mut published_now = HashSet::new();
        for (file, diagnostics) in &per_file {
            let uri = path_to_uri(Path::new(file));
            let items: Vec<Value> = diagnostics
                .iter()
                .map(|diagnostic| self.to_lsp_diagnostic(Path::new(file), diagnostic))
                .collect();
            published_now.insert(uri.clone());
            messages.push(notification(
                "textDocument/publishDiagnostics",
                json!({"uri": uri, "diagnostics": items}),
            ));
        }
        // Clear diagnostics for files that had some before but not anymore.
        for stale in self.published_uris.difference(&published_now) {
            messages.push(notification(
                "textDocument/publishDiagnostics",
                json!({"uri": stale, "diagnostics": []}),
            ));
        }
        self.published_uris = published_now;
        messages
    }

    fn to_lsp_diagnostic(&self, file: &Path, diagnostic: &Diagnostic) -> Value {
        let range = diagnostic
            .span()
            .and_then(|span| {
                let text = self.file_text(file)?;
                Some(json!({
                    "start": position(&text, span.start as usize),
                    "end": position(&text, span.end as usize),
                }))
            })
            .unwrap_or_else(|| {
                json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}})
            });
        json!({
            "range": range,
            "severity": match diagnostic.severity() {
                Severity::Error => 1,
                Severity::Warning => 2,
            },
            "source": "waluau",
            "message": diagnostic.to_string(),
        })
    }

    fn file_text(&self, path: &Path) -> Option<String> {
        if let Some(text) = self.open_documents.get(path) {
            return Some(text.clone());
        }
        std::fs::read_to_string(path).ok()
    }
}

/// Byte offset in `text` to an LSP position (zero-based line, UTF-16 column).
fn position(text: &str, byte_offset: usize) -> Value {
    let clamped = byte_offset.min(text.len());
    let mut line = 0u32;
    let mut character = 0u32;
    for (index, ch) in text.char_indices() {
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
    json!({"line": line, "character": character})
}

fn document_path(params: &Value) -> Option<(PathBuf, String)> {
    let uri = params["textDocument"]["uri"].as_str()?;
    Some((uri_to_path(uri)?, uri.to_string()))
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    // Minimal percent-decoding for the characters editors commonly escape.
    let mut decoded = String::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    let mut buffer = Vec::new();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next()?;
            let low = bytes.next()?;
            let hex = [high, low];
            let hex = std::str::from_utf8(&hex).ok()?;
            buffer.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            if !buffer.is_empty() {
                decoded.push_str(std::str::from_utf8(&buffer).ok()?);
                buffer.clear();
            }
            decoded.push(byte as char);
        }
    }
    if !buffer.is_empty() {
        decoded.push_str(std::str::from_utf8(&buffer).ok()?);
    }
    Some(PathBuf::from(decoded))
}

fn path_to_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn response(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

fn notification(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

#[cfg(test)]
mod tests;
