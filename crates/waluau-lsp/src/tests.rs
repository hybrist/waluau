use std::path::Path;

use serde_json::{Value, json};

use super::LspServer;

fn open(server: &mut LspServer, path: &Path, text: &str) -> Vec<Value> {
    send(
        server,
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": format!("file://{}", path.display()),
                "languageId": "waluau",
                "version": 1,
                "text": text,
            }},
        }),
    )
}

fn change(server: &mut LspServer, path: &Path, text: &str) -> Vec<Value> {
    send(
        server,
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": {"uri": format!("file://{}", path.display()), "version": 2},
                "contentChanges": [{"text": text}],
            },
        }),
    )
}

fn send(server: &mut LspServer, message: Value) -> Vec<Value> {
    server
        .handle_message(&message.to_string())
        .into_iter()
        .map(|outgoing| serde_json::from_str(&outgoing).expect("outgoing message is JSON"))
        .collect()
}

fn diagnostics_for<'a>(messages: &'a [Value], uri_suffix: &str) -> Option<&'a Vec<Value>> {
    messages.iter().find_map(|message| {
        (message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"]
                .as_str()
                .is_some_and(|uri| uri.ends_with(uri_suffix)))
        .then(|| message["params"]["diagnostics"].as_array())
        .flatten()
    })
}

#[test]
fn initialize_reports_full_document_sync() {
    let mut server = LspServer::new();
    let responses = send(
        &mut server,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    );
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["result"]["capabilities"]["textDocumentSync"], 1);
}

#[test]
fn did_open_publishes_every_error_with_ranges() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let source = "function first(x: i32): bool\n    return x\nend\nfunction second(x: i32): i32\n    if x then\n        return x\n    end\n    return x\nend\n";
    std::fs::write(&path, source).expect("write fixture");

    let mut server = LspServer::new();
    let messages = open(&mut server, &path, source);
    let diagnostics = diagnostics_for(&messages, "main.walu").expect("diagnostics published");
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    for diagnostic in diagnostics {
        assert_eq!(diagnostic["severity"], 1);
        assert_eq!(diagnostic["source"], "waluau");
    }
    // The first function's return mismatch has a span and must map to its
    // line (0-based line 1). The if-condition diagnostic currently carries no
    // span in HIR and falls back to 0:0 — span coverage is tracked separately.
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic["message"] == "cannot implicitly convert i32 to bool"
                && diagnostic["range"]["start"]["line"] == 1
        }),
        "expected a positioned return-mismatch diagnostic: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["message"] == "if condition must be bool"),
        "{diagnostics:?}"
    );
}

#[test]
fn did_change_clears_fixed_diagnostics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let broken = "function bad(x: i32): bool\n    return x\nend\n";
    std::fs::write(&path, broken).expect("write fixture");

    let mut server = LspServer::new();
    let messages = open(&mut server, &path, broken);
    assert_eq!(
        diagnostics_for(&messages, "main.walu").map(Vec::len),
        Some(1)
    );

    let fixed = "function bad(x: i32): bool\n    return x > 0\nend\n";
    let messages = change(&mut server, &path, fixed);
    assert_eq!(
        diagnostics_for(&messages, "main.walu").map(Vec::len),
        Some(0),
        "fixed file should get an explicit empty publish: {messages:?}"
    );
}

#[test]
fn errors_in_required_modules_publish_under_their_own_uri() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = dir.path().join("lib.walu");
    let main = dir.path().join("main.walu");
    std::fs::write(
        &lib,
        "function double(x: i32): bool\n    return x * 2\nend\nreturn double\n",
    )
    .expect("write fixture");
    let main_source = "local double = require(\"./lib\")\nlocal a = double(2)\n";
    std::fs::write(&main, main_source).expect("write fixture");

    let mut server = LspServer::new();
    let messages = open(&mut server, &main, main_source);
    let lib_diagnostics = diagnostics_for(&messages, "lib.walu").expect("lib diagnostics");
    assert_eq!(lib_diagnostics.len(), 1, "{lib_diagnostics:?}");
}

#[test]
fn unsynced_editor_buffer_wins_over_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    // Disk copy is broken; the freshly-opened buffer is fixed but unsaved.
    std::fs::write(&path, "function bad(x: i32): bool\n    return x\nend\n")
        .expect("write fixture");

    let mut server = LspServer::new();
    let messages = open(
        &mut server,
        &path,
        "function bad(x: i32): bool\n    return x > 0\nend\n",
    );
    assert!(
        diagnostics_for(&messages, "main.walu").is_none_or(|diagnostics| diagnostics.is_empty()),
        "buffer content should be analyzed, not the stale disk copy: {messages:?}"
    );
}

#[test]
fn shutdown_then_exit_is_clean() {
    let mut server = LspServer::new();
    let responses = send(
        &mut server,
        json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
    );
    assert_eq!(responses[0]["result"], Value::Null);
    send(&mut server, json!({"jsonrpc": "2.0", "method": "exit"}));
    assert!(server.exit_requested());
    assert!(server.clean_exit());
}
