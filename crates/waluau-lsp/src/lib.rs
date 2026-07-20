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
use waluau_fmt::{FormatConfig, format_source};

mod features;

/// Diagnostics an [`AnalysisBackend`] produced for one analysis root.
pub struct BackendAnalysis {
    pub diagnostics: Vec<Diagnostic>,
}

/// Analysis engine behind the protocol core. The native server uses a
/// [`CompilerSession`] (filesystem + overlays); the wasm build plugs in an
/// in-memory backend so the same core runs client-side in the playground.
pub trait AnalysisBackend {
    /// Register `content` as the live buffer for `path`. The backend owns
    /// path normalization (e.g. canonicalization against the filesystem).
    fn set_overlay(&mut self, path: &Path, content: &str);
    fn remove_overlay(&mut self, path: &Path);
    fn analyze_root(&mut self, root: &Path) -> BackendAnalysis;
}

impl AnalysisBackend for CompilerSession {
    fn set_overlay(&mut self, path: &Path, content: &str) {
        // The linker resolves modules through canonical paths; the overlay
        // must be registered under the same key the resolver produces.
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        CompilerSession::set_overlay(self, canonical, content);
    }

    fn remove_overlay(&mut self, path: &Path) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        CompilerSession::remove_overlay(self, &canonical);
    }

    fn analyze_root(&mut self, root: &Path) -> BackendAnalysis {
        BackendAnalysis {
            diagnostics: CompilerSession::analyze_root(self, root).diagnostics,
        }
    }
}

pub struct LspServer<B: AnalysisBackend = CompilerSession> {
    backend: B,
    /// Open documents by filesystem path, holding the latest buffer text.
    open_documents: HashMap<PathBuf, String>,
    /// URIs we last published diagnostics for, so stale ones can be cleared.
    published_uris: HashSet<String>,
    shutdown_requested: bool,
    exit_requested: bool,
}

impl Default for LspServer<CompilerSession> {
    fn default() -> Self {
        Self::new()
    }
}

impl LspServer<CompilerSession> {
    pub fn new() -> Self {
        Self::with_backend(CompilerSession::new())
    }
}

impl<B: AnalysisBackend> LspServer<B> {
    pub fn with_backend(backend: B) -> Self {
        Self {
            backend,
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
                return vec![error_response(
                    Value::Null,
                    -32700,
                    &format!("parse error: {error}"),
                )];
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
                        "hoverProvider": true,
                        "definitionProvider": true,
                        "completionProvider": {"triggerCharacters": [".", ":"]},
                        "documentFormattingProvider": true,
                        "documentRangeFormattingProvider": true,
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
                self.backend.remove_overlay(&path);
                self.publish_all_diagnostics()
            }
            ("textDocument/didSave", _) => Vec::new(),
            ("textDocument/hover", Some(id)) => vec![self.handle_hover(id, &params)],
            ("textDocument/definition", Some(id)) => vec![self.handle_definition(id, &params)],
            ("textDocument/completion", Some(id)) => vec![self.handle_completion(id, &params)],
            ("textDocument/formatting", Some(id)) => vec![self.handle_formatting(id, &params)],
            ("textDocument/rangeFormatting", Some(id)) => {
                vec![self.handle_range_formatting(id, &params)]
            }
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
        self.backend.set_overlay(&path, &text);
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
            let analysis = self.backend.analyze_root(root);
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

    /// The document and byte offset a position-based request points at.
    fn request_document(&self, params: &Value) -> Option<(PathBuf, String, u32)> {
        let (path, _uri) = document_path(params)?;
        let text = self.file_text(&path)?;
        let line = params["position"]["line"].as_u64()? as u32;
        let character = params["position"]["character"].as_u64()? as u32;
        let offset = offset_at(&text, line, character);
        Some((path, text, offset))
    }

    /// Loader for cross-file lookups: open buffers win over the filesystem.
    /// Open documents are keyed by the URI-derived path, so a canonicalized
    /// candidate is tried both ways before falling back to disk (which the
    /// wasm build simply fails, limiting it to open documents).
    fn workspace_loader(&self) -> impl Fn(&Path) -> Option<String> + '_ {
        |path: &Path| {
            if let Some(text) = self.open_documents.get(path) {
                return Some(text.clone());
            }
            if let Ok(canonical) = path.canonicalize()
                && let Some(text) = self.open_documents.get(&canonical)
            {
                return Some(text.clone());
            }
            std::fs::read_to_string(path).ok()
        }
    }

    fn handle_hover(&self, id: Value, params: &Value) -> String {
        let Some((path, text, offset)) = self.request_document(params) else {
            return response(id, Value::Null);
        };
        let load = self.workspace_loader();
        match features::hover(&text, &path, offset, &load) {
            Some(hover) => response(
                id,
                json!({
                    "contents": {"kind": "markdown", "value": hover.contents},
                    "range": {
                        "start": position(&text, hover.span.start as usize),
                        "end": position(&text, hover.span.end as usize),
                    },
                }),
            ),
            None => response(id, Value::Null),
        }
    }

    fn handle_definition(&self, id: Value, params: &Value) -> String {
        let Some((path, text, offset)) = self.request_document(params) else {
            return response(id, Value::Null);
        };
        let load = self.workspace_loader();
        let Some((target_file, span)) = features::definition(&text, &path, offset, &load) else {
            return response(id, Value::Null);
        };
        // The span is a byte range in the *target* file's current text.
        let target_text = if target_file == path {
            text
        } else {
            match load(&target_file) {
                Some(text) => text,
                None => return response(id, Value::Null),
            }
        };
        response(
            id,
            json!({
                "uri": path_to_uri(&target_file),
                "range": {
                    "start": position(&target_text, span.start as usize),
                    "end": position(&target_text, span.end as usize),
                },
            }),
        )
    }

    fn handle_completion(&self, id: Value, params: &Value) -> String {
        let Some((path, text, offset)) = self.request_document(params) else {
            return response(id, Value::Null);
        };
        let load = self.workspace_loader();
        let items: Vec<Value> = features::completion(&text, &path, offset, &load)
            .into_iter()
            .map(|item| {
                json!({
                    "label": item.label,
                    "kind": item.kind,
                    "detail": item.detail,
                })
            })
            .collect();
        response(id, json!(items))
    }

    /// `textDocument/formatting`: reformat the whole document. Returns a single
    /// full-document edit, or no edits when the document is already formatted
    /// or does not parse.
    fn handle_formatting(&self, id: Value, params: &Value) -> String {
        let Some((path, _uri)) = document_path(params) else {
            return response(id, Value::Null);
        };
        let Some(text) = self.file_text(&path) else {
            return response(id, json!([]));
        };
        let Ok(formatted) = format_source(&text, &FormatConfig::default()) else {
            // Invalid source: nothing to format.
            return response(id, json!([]));
        };
        if formatted == text {
            return response(id, json!([]));
        }
        let edit = json!({
            "range": {
                "start": {"line": 0, "character": 0},
                "end": position(&text, text.len()),
            },
            "newText": formatted,
        });
        response(id, json!([edit]))
    }

    /// `textDocument/rangeFormatting`: reformat, but only emit an edit for the
    /// minimal block of changed lines, and only when it overlaps the requested
    /// range. The formatter needs whole-file context to parse, so it formats
    /// the document and returns a line-granular diff scoped to the request.
    fn handle_range_formatting(&self, id: Value, params: &Value) -> String {
        let Some((path, _uri)) = document_path(params) else {
            return response(id, Value::Null);
        };
        let Some(text) = self.file_text(&path) else {
            return response(id, json!([]));
        };
        let start_line = params["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
        let end_line = params["range"]["end"]["line"].as_u64().unwrap_or(u64::MAX) as usize;

        let Ok(formatted) = format_source(&text, &FormatConfig::default()) else {
            return response(id, json!([]));
        };
        let Some(edit) = line_diff_edit(&text, &formatted) else {
            return response(id, json!([])); // already formatted
        };
        // Only act when the changed block overlaps the requested line range.
        if edit.start_line > end_line || edit.end_line <= start_line {
            return response(id, json!([]));
        }
        let value = json!({
            "range": {
                "start": {"line": edit.start_line, "character": 0},
                "end": {"line": edit.end_line, "character": 0},
            },
            "newText": edit.new_text,
        });
        response(id, json!([value]))
    }
}

/// The minimal line-range edit turning `original` into `formatted`: the block
/// of lines `[start_line, end_line)` to replace, and its replacement text
/// (newline-terminated). `None` when the two are identical. Computed by
/// trimming equal leading and trailing lines so the edit is as small as
/// possible.
struct LineEdit {
    start_line: usize,
    end_line: usize,
    new_text: String,
}

fn line_diff_edit(original: &str, formatted: &str) -> Option<LineEdit> {
    if original == formatted {
        return None;
    }
    // `lines()` drops the trailing newline; include an explicit final empty
    // segment so full-line ranges map cleanly to `{line, character: 0}`.
    let orig: Vec<&str> = original.split('\n').collect();
    let fmt: Vec<&str> = formatted.split('\n').collect();

    let mut prefix = 0;
    while prefix < orig.len() && prefix < fmt.len() && orig[prefix] == fmt[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < orig.len() - prefix
        && suffix < fmt.len() - prefix
        && orig[orig.len() - 1 - suffix] == fmt[fmt.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let start_line = prefix;
    let end_line = orig.len() - suffix; // exclusive
    let replacement_lines = &fmt[prefix..fmt.len() - suffix];
    // Each replaced source line owns its trailing newline (the edit range ends
    // at the start of `end_line`), so terminate every replacement line too.
    let mut new_text = String::new();
    for line in replacement_lines {
        new_text.push_str(line);
        new_text.push('\n');
    }
    Some(LineEdit {
        start_line,
        end_line,
        new_text,
    })
}

/// LSP position (zero-based line, UTF-16 column) to a byte offset in `text`;
/// the inverse of [`position`]. Positions past a line's end clamp to the line
/// break; positions past the last line clamp to the text's end.
fn offset_at(text: &str, line: u32, character: u32) -> u32 {
    let mut current_line = 0u32;
    let mut utf16_column = 0u32;
    for (index, ch) in text.char_indices() {
        if current_line == line {
            if utf16_column >= character || ch == '\n' {
                return index as u32;
            }
            utf16_column += ch.len_utf16() as u32;
        } else if ch == '\n' {
            current_line += 1;
        }
    }
    text.len() as u32
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
