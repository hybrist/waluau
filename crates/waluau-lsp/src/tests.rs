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
    assert_eq!(
        responses[0]["result"]["capabilities"]["textDocumentSync"],
        1
    );
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
    // The first function's return mismatch maps to its line (0-based line
    // 1); the if-condition error maps to the condition on line 4.
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic["message"] == "cannot implicitly convert i32 to bool"
                && diagnostic["range"]["start"]["line"] == 1
        }),
        "expected a positioned return-mismatch diagnostic: {diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic["message"] == "if condition must be bool"
                && diagnostic["range"]["start"]["line"] == 4
        }),
        "expected the if-condition diagnostic on its own line: {diagnostics:?}"
    );
}

fn formatting_request(server: &mut LspServer, method: &str, path: &Path, range: Value) -> Value {
    let mut params = json!({
        "textDocument": {"uri": format!("file://{}", path.display())},
        "options": {"tabSize": 4, "insertSpaces": true},
    });
    if !range.is_null() {
        params["range"] = range;
    }
    let responses = send(
        server,
        json!({"jsonrpc": "2.0", "id": 9, "method": method, "params": params}),
    );
    responses[0]["result"].clone()
}

#[test]
fn initialize_advertises_formatting_providers() {
    let mut server = LspServer::new();
    let responses = send(
        &mut server,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    );
    let caps = &responses[0]["result"]["capabilities"];
    assert_eq!(caps["documentFormattingProvider"], true);
    assert_eq!(caps["documentRangeFormattingProvider"], true);
}

#[test]
fn document_formatting_returns_full_document_edit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let messy = "function  f(a:i32):i32\nreturn a\nend\n";
    std::fs::write(&path, messy).expect("write");

    let mut server = LspServer::new();
    open(&mut server, &path, messy);
    let result = formatting_request(&mut server, "textDocument/formatting", &path, Value::Null);
    let edits = result.as_array().expect("edits array");
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0]["newText"],
        "function f(a: i32): i32\n    return a\nend\n"
    );
    assert_eq!(
        edits[0]["range"]["start"],
        json!({"line": 0, "character": 0})
    );
}

#[test]
fn document_formatting_of_clean_file_returns_no_edits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let clean = "local x = 1\n";
    std::fs::write(&path, clean).expect("write");

    let mut server = LspServer::new();
    open(&mut server, &path, clean);
    let result = formatting_request(&mut server, "textDocument/formatting", &path, Value::Null);
    assert_eq!(result, json!([]));
}

#[test]
fn range_formatting_only_touches_overlapping_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    // Line 0 is clean; line 1 is messy.
    let source = "local x = 1\nlocal    y=2\n";
    std::fs::write(&path, source).expect("write");

    let mut server = LspServer::new();
    open(&mut server, &path, source);

    // A range on the messy line yields an edit for just that line.
    let range = json!({"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 0}});
    let result = formatting_request(&mut server, "textDocument/rangeFormatting", &path, range);
    let edits = result.as_array().expect("edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["newText"], "local y = 2\n");
    assert_eq!(edits[0]["range"]["start"]["line"], 1);

    // A range on the already-clean line yields no edit.
    let range = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}});
    let result = formatting_request(&mut server, "textDocument/rangeFormatting", &path, range);
    assert_eq!(result, json!([]));
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

fn request(server: &mut LspServer, method: &str, path: &Path, line: u32, character: u32) -> Value {
    let responses = send(
        server,
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": method,
            "params": {
                "textDocument": {"uri": format!("file://{}", path.display())},
                "position": {"line": line, "character": character},
            },
        }),
    );
    responses[0]["result"].clone()
}

/// Zero-based (line, character) of `needle` in `text` (first occurrence).
fn position_of(text: &str, needle: &str) -> (u32, u32) {
    let offset = text.find(needle).expect("needle should exist");
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let character = before.rsplit('\n').next().unwrap_or(before).len() as u32;
    (line, character)
}

#[test]
fn initialize_advertises_language_features() {
    let mut server = LspServer::new();
    let responses = send(
        &mut server,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    );
    let capabilities = &responses[0]["result"]["capabilities"];
    assert_eq!(capabilities["hoverProvider"], json!(true));
    assert_eq!(capabilities["definitionProvider"], json!(true));
    assert_eq!(
        capabilities["completionProvider"]["triggerCharacters"],
        json!([".", ":"])
    );
}

#[test]
fn hover_reports_local_types_and_function_signatures() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "function add(a: i32, b: i32): i32\n    return a + b\nend\nlocal total: i32 = add(1, 2)\nprint(tostring(total))\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    // Hover the `total` reference in the final print line.
    let (line, character) = position_of(text, "total))");
    let hover = request(&mut server, "textDocument/hover", &path, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("local total: i32"), "{contents}");

    // Hover the `add` call.
    let (line, character) = position_of(text, "add(1, 2)");
    let hover = request(&mut server, "textDocument/hover", &path, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(
        contents.contains("function add(a: i32, b: i32): i32"),
        "{contents}"
    );

    // Hover a parameter use inside the body.
    let (line, character) = position_of(text, "a + b");
    let hover = request(&mut server, "textDocument/hover", &path, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("(parameter) a: i32"), "{contents}");
}

#[test]
fn hover_resolves_builtin_namespace_members() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "local x: f64 = math.floor(1.5)\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    let (line, character) = position_of(text, "floor");
    let hover = request(&mut server, "textDocument/hover", &path, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("math.floor"), "{contents}");
}

#[test]
fn definition_jumps_to_the_binding_site() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "local answer: i32 = 42\nprint(tostring(answer))\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    let (line, character) = position_of(text, "answer))");
    let location = request(
        &mut server,
        "textDocument/definition",
        &path,
        line,
        character,
    );
    assert!(
        location["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("main.walu")),
        "{location:?}"
    );
    assert_eq!(location["range"]["start"]["line"], json!(0));
    assert_eq!(location["range"]["start"]["character"], json!(6));
    assert_eq!(location["range"]["end"]["character"], json!(12));
}

#[test]
fn definition_shadowing_picks_the_innermost_binding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "local x = 1\ndo\n    local x = 2\n    print(tostring(x))\nend\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    let (line, character) = position_of(text, "x))");
    let location = request(
        &mut server,
        "textDocument/definition",
        &path,
        line,
        character,
    );
    assert_eq!(location["range"]["start"]["line"], json!(2), "{location:?}");
}

#[test]
fn definition_crosses_into_required_modules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = dir.path().join("lib.walu");
    let lib_text = "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n";
    std::fs::write(&lib, lib_text).expect("write fixture");
    let main = dir.path().join("main.walu");
    let main_text = "local m = require(\"./lib\")\nlocal y: i32 = m.double(4)\n";
    std::fs::write(&main, main_text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &main, main_text);

    let (line, character) = position_of(main_text, "double(4)");
    let location = request(
        &mut server,
        "textDocument/definition",
        &main,
        line,
        character,
    );
    assert!(
        location["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("lib.walu")),
        "{location:?}"
    );
    assert_eq!(location["range"]["start"]["line"], json!(0));

    // Hover across the module boundary shows the target signature.
    let hover = request(&mut server, "textDocument/hover", &main, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(
        contents.contains("function double(x: i32): i32"),
        "{contents}"
    );
}

#[test]
fn completion_lists_visible_scope_and_keywords() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "function add(a: i32, b: i32): i32\n    local partial: i32 = a\n    return partial\nend\nlocal outer = 1\n\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    // Inside the function body: params and the local are in scope.
    let (line, character) = position_of(text, "return partial");
    let result = request(
        &mut server,
        "textDocument/completion",
        &path,
        line,
        character + "return ".len() as u32 + "partial".len() as u32,
    );
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"partial"), "{labels:?}");
    assert!(labels.contains(&"a"), "{labels:?}");
    assert!(labels.contains(&"add"), "{labels:?}");
    assert!(labels.contains(&"print"), "{labels:?}");
    assert!(labels.contains(&"math"), "{labels:?}");
    assert!(labels.contains(&"function"), "{labels:?}");

    // At the file tail, function-scoped names are gone but globals remain.
    let last_line = text.matches('\n').count() as u32;
    let result = request(&mut server, "textDocument/completion", &path, last_line, 0);
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(!labels.contains(&"partial"), "{labels:?}");
    assert!(labels.contains(&"outer"), "{labels:?}");
    assert!(labels.contains(&"add"), "{labels:?}");
}

#[test]
fn completion_after_dot_lists_namespace_and_module_members() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = dir.path().join("lib.walu");
    std::fs::write(
        &lib,
        "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n",
    )
    .expect("write fixture");
    let main = dir.path().join("main.walu");
    let main_text = "local m = require(\"./lib\")\nlocal x: f64 = math.\n";
    std::fs::write(&main, main_text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &main, main_text);

    // `math.` members come from the builtins prelude.
    let (line, character) = position_of(main_text, "math.");
    let result = request(
        &mut server,
        "textDocument/completion",
        &main,
        line,
        character + "math.".len() as u32,
    );
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"floor"), "{labels:?}");
    assert!(labels.contains(&"pi"), "{labels:?}");
    assert!(
        !labels.iter().any(|label| label.contains('.')),
        "{labels:?}"
    );

    // Members of a required module: append `m.` and complete after it.
    let extended = format!("{main_text}m.");
    change(&mut server, &main, &extended);
    let last_line = extended.matches('\n').count() as u32;
    let result = request(&mut server, "textDocument/completion", &main, last_line, 2);
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert_eq!(labels, vec!["double"], "{labels:?}");
}

#[test]
fn completion_after_colon_offers_type_names_in_annotations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "type Point = {f64}\nlocal p: \n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    let (line, character) = position_of(text, "local p: ");
    let result = request(
        &mut server,
        "textDocument/completion",
        &path,
        line,
        character + "local p: ".len() as u32,
    );
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"i32"), "{labels:?}");
    assert!(labels.contains(&"Point"), "{labels:?}");
}

/// Fixture mirroring the ante game module shape: a record type, a
/// constructor with a declared return type, and a method.
const GAME_MODULE: &str = "type Card = { rank: i32, suit: i32 }\n\
type State = { middle: {Card}, round: i32 }\n\
function State:play(first: i32, second: i32): unit\n\
end\n\
function new(): State\n\
    return { middle = {}, round = 1 }\n\
end\n\
return new\n";

const GAME_TEST: &str = "local game = require(\"./game\")\n\
local state = game.new()\n\
local marked_card = state.middle[0]\n\
state:play(0, marked_card.rank)\n\
print(tostring(state.round))\n";

fn game_workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("game.walu"), GAME_MODULE).expect("write fixture");
    let main = dir.path().join("sim.test.walu");
    std::fs::write(&main, GAME_TEST).expect("write fixture");
    (dir, main)
}

#[test]
fn member_hover_follows_inferred_local_types_across_modules() {
    let (_dir, main) = game_workspace();
    let mut server = LspServer::new();
    open(&mut server, &main, GAME_TEST);

    // `state.middle` — `state` is unannotated, inferred from game.new().
    let (line, character) = position_of(GAME_TEST, "middle[0]");
    let hover = request(&mut server, "textDocument/hover", &main, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("middle: {Card}"), "{contents}");

    // The hovered base itself shows its inferred type.
    let (line, character) = position_of(GAME_TEST, "state.middle");
    let hover = request(&mut server, "textDocument/hover", &main, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("local state: State"), "{contents}");

    // A local initialized from an indexed field read gets the element type.
    let (line, character) = position_of(GAME_TEST, "marked_card.rank");
    let hover = request(&mut server, "textDocument/hover", &main, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("local marked_card: Card"), "{contents}");

    // And members of that element type resolve too.
    let (line, character) = position_of(GAME_TEST, "rank)");
    let hover = request(&mut server, "textDocument/hover", &main, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("rank: i32"), "{contents}");
}

#[test]
fn member_definition_jumps_to_the_type_declaration() {
    let (_dir, main) = game_workspace();
    let mut server = LspServer::new();
    open(&mut server, &main, GAME_TEST);

    let (line, character) = position_of(GAME_TEST, "middle[0]");
    let location = request(
        &mut server,
        "textDocument/definition",
        &main,
        line,
        character,
    );
    assert!(
        location["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("game.walu")),
        "{location:?}"
    );
    // `type State = ...` is on line 2 (zero-based 1) of the module.
    assert_eq!(location["range"]["start"]["line"], json!(1), "{location:?}");
}

#[test]
fn method_hover_and_definition_resolve_through_the_receiver_type() {
    let (_dir, main) = game_workspace();
    let mut server = LspServer::new();
    open(&mut server, &main, GAME_TEST);

    let (line, character) = position_of(GAME_TEST, "play(0");
    let hover = request(&mut server, "textDocument/hover", &main, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(
        contents.contains("function State:play(first: i32, second: i32): unit"),
        "{contents}"
    );

    let location = request(
        &mut server,
        "textDocument/definition",
        &main,
        line,
        character,
    );
    assert!(
        location["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("game.walu")),
        "{location:?}"
    );
    assert_eq!(location["range"]["start"]["line"], json!(2), "{location:?}");
}

#[test]
fn member_completion_offers_fields_and_methods_of_the_inferred_type() {
    let (_dir, main) = game_workspace();
    let mut server = LspServer::new();
    open(&mut server, &main, GAME_TEST);

    // Complete after `state.` on its own line.
    let extended = format!("{GAME_TEST}state.");
    change(&mut server, &main, &extended);
    let last_line = extended.matches('\n').count() as u32;
    let result = request(&mut server, "textDocument/completion", &main, last_line, 6);
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"middle"), "{labels:?}");
    assert!(labels.contains(&"round"), "{labels:?}");
    assert!(labels.contains(&"play"), "{labels:?}");

    // After `state:` only methods remain.
    let extended = format!("{GAME_TEST}state:");
    change(&mut server, &main, &extended);
    let result = request(&mut server, "textDocument/completion", &main, last_line, 6);
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"play"), "{labels:?}");
    assert!(!labels.contains(&"middle"), "{labels:?}");
}

#[test]
fn annotated_module_qualified_types_resolve_members() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("game.walu"), GAME_MODULE).expect("write fixture");
    let text = "local game = require(\"./game\")\n\
local function reset(state: game.State): unit\n\
    print(tostring(state.round))\n\
end\n";
    let main = dir.path().join("main.walu");
    std::fs::write(&main, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &main, text);

    let (line, character) = position_of(text, "round))");
    let hover = request(&mut server, "textDocument/hover", &main, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("round: i32"), "{contents}");
}

#[test]
fn table_literal_locals_offer_fields_and_value_methods() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "local point = { x = 10::i32, y = 3::i32 }\n\
function point:bump_x(delta: i32): i32\n\
    return delta\n\
end\n\
assert(point.x == 10::i32)\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    // Hover the field read: the table literal's cast reveals the type.
    let (line, character) = position_of(text, "x == 10");
    let hover = request(&mut server, "textDocument/hover", &path, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("x: i32"), "{contents}");

    // `point.` offers fields and value-level methods together.
    let extended = format!("{text}point.");
    change(&mut server, &path, &extended);
    let last_line = extended.matches('\n').count() as u32;
    let result = request(&mut server, "textDocument/completion", &path, last_line, 6);
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"x"), "{labels:?}");
    assert!(labels.contains(&"y"), "{labels:?}");
    assert!(labels.contains(&"bump_x"), "{labels:?}");

    // `point:` offers only the methods.
    let extended = format!("{text}point:");
    change(&mut server, &path, &extended);
    let result = request(&mut server, "textDocument/completion", &path, last_line, 6);
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"bump_x"), "{labels:?}");
    assert!(!labels.contains(&"x"), "{labels:?}");
    assert!(
        !labels.contains(&"i32"),
        "type names should not appear: {labels:?}"
    );
}
