use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{Value, json};

use super::{AnalysisBackend, BackendAnalysis, LspServer};

#[derive(Default)]
struct BackendCalls {
    overlays_set: Vec<PathBuf>,
    overlays_removed: Vec<PathBuf>,
    roots_analyzed: Vec<PathBuf>,
}

struct CountingBackend(Rc<RefCell<BackendCalls>>);

impl AnalysisBackend for CountingBackend {
    fn set_overlay(&mut self, path: &Path, _content: &str) {
        self.0.borrow_mut().overlays_set.push(path.to_path_buf());
    }

    fn remove_overlay(&mut self, path: &Path) {
        self.0
            .borrow_mut()
            .overlays_removed
            .push(path.to_path_buf());
    }

    fn analyze_root(&mut self, root: &Path) -> BackendAnalysis {
        self.0.borrow_mut().roots_analyzed.push(root.to_path_buf());
        BackendAnalysis {
            diagnostics: Vec::new(),
        }
    }
}

#[test]
fn batched_document_replacement_does_not_analyze_or_reset_unchanged_overlays() {
    let calls = Rc::new(RefCell::new(BackendCalls::default()));
    let mut server = LspServer::with_backend(CountingBackend(Rc::clone(&calls)));
    server.replace_documents(HashMap::from([
        (PathBuf::from("/main.walu"), "local x = 1".to_string()),
        (PathBuf::from("/lib.walu"), "return 1".to_string()),
    ]));
    server.replace_documents(HashMap::from([
        (PathBuf::from("/main.walu"), "local x = 2".to_string()),
        (PathBuf::from("/new.walu"), "return 2".to_string()),
    ]));

    let calls = calls.borrow();
    assert_eq!(calls.roots_analyzed, Vec::<PathBuf>::new());
    assert_eq!(calls.overlays_set.len(), 4);
    assert_eq!(
        calls
            .overlays_set
            .iter()
            .filter(|path| path.as_path() == Path::new("/main.walu"))
            .count(),
        2
    );
    assert!(calls.overlays_set.contains(&PathBuf::from("/lib.walu")));
    assert!(calls.overlays_set.contains(&PathBuf::from("/new.walu")));
    assert_eq!(calls.overlays_removed, vec![PathBuf::from("/lib.walu")]);
}

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
    assert_eq!(capabilities["referencesProvider"], json!(true));
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
fn exported_type_definitions_cross_module_namespace() {
    let cases: Vec<(&str, Vec<(&'static str, &str)>)> = vec![
        (
            "exported enum in a type annotation",
            vec![
                (
                    "spells/spell.walu",
                    "export enum <def>SpellKind</def> { Firebolt, FreezeRay }\n",
                ),
                (
                    "spell_launch.walu",
                    "local spells = require(\"./spells/spell\")\nfunction name(kind: spells.<|>SpellKind): string\n    return \"spell\"\nend\n",
                ),
            ],
        ),
        (
            "exported type alias in a type annotation",
            vec![
                (
                    "model.walu",
                    "export type <def>Point</def> = { x: i32, y: i32 }\n",
                ),
                (
                    "main.walu",
                    "local model = require(\"./model\")\nlocal point: model.<|>Point = { x = 1, y = 2 }\n",
                ),
            ],
        ),
    ];

    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(name, files)| definition_case(name, files))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
        "export type Public = { value: i32 }\n\
type Hidden = { value: i32 }\n\
export enum Direction { north, south }\n\
function double(x: i32): i32\n    return x * 2\nend\nreturn double\n",
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
    assert!(labels.contains(&"double"), "{labels:?}");
    assert!(labels.contains(&"Public"), "{labels:?}");
    assert!(labels.contains(&"Direction"), "{labels:?}");
    assert!(!labels.contains(&"Hidden"), "{labels:?}");
}

#[test]
fn exported_enum_variant_definition_crosses_module_namespace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = dir.path().join("directions.walu");
    let lib_text = "type Hidden = i32\nexport enum Direction { north, south }\n";
    std::fs::write(&lib, lib_text).expect("write fixture");
    let main = dir.path().join("main.walu");
    let text = "local directions = require(\"./directions\")\n\
local direction: directions.Direction = directions.Direction.north\n";
    std::fs::write(&main, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &main, text);
    let (line, character) = position_of(text, "north");
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
            .is_some_and(|uri| uri.ends_with("directions.walu")),
        "{location:?}"
    );
    assert_eq!(location["range"]["start"]["line"], json!(1));
}

#[test]
fn completion_after_dot_lists_enum_variants() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "enum SpellType {\n    Fireball,\n    Freeze,\n    RaiseCard,\n    Clone,\n}\nlocal t = SpellType.\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    let (line, character) = position_of(text, "SpellType.");
    let result = request(
        &mut server,
        "textDocument/completion",
        &path,
        line,
        character + "SpellType.".len() as u32,
    );
    let items = result.as_array().expect("completion items");
    let labels: Vec<&str> = items
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert_eq!(labels, vec!["Clone", "Fireball", "Freeze", "RaiseCard"]);
    let fireball = items
        .iter()
        .find(|item| item["label"] == "Fireball")
        .expect("Fireball item");
    assert_eq!(
        fireball["detail"].as_str(),
        Some("SpellType.Fireball: SpellType")
    );
}

#[test]
fn completion_chases_dotted_receivers_through_module_aliases() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = dir.path().join("lib.walu");
    std::fs::write(
        &lib,
        "export enum SpellType { Fireball, Freeze }\n\
export type Point = { x: i32, y: i32 }\n\
function Point.new(): Point\n    return { x = 0, y = 0 }\nend\n\
function Point:shift(dx: i32): unit\nend\n",
    )
    .expect("write fixture");
    let main = dir.path().join("main.walu");
    let base = "local m = require(\"./lib\")\n";
    std::fs::write(&main, base).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &main, base);
    let last_line = base.matches('\n').count() as u32;
    let mut complete_after = |text: &str| -> Vec<String> {
        let extended = format!("{base}{text}");
        change(&mut server, &main, &extended);
        let result = request(
            &mut server,
            "textDocument/completion",
            &main,
            last_line,
            text.len() as u32,
        );
        result
            .as_array()
            .expect("completion items")
            .iter()
            .filter_map(|item| item["label"].as_str().map(str::to_string))
            .collect()
    };

    // Variants of an enum reached through the require alias.
    let labels = complete_after("local t = m.SpellType.");
    assert_eq!(labels, vec!["Fireball", "Freeze"]);

    // Members hung off an exported type namespace behind the alias.
    let labels = complete_after("local p = m.Point.");
    assert!(labels.contains(&"new".to_string()), "{labels:?}");
    assert!(labels.contains(&"shift".to_string()), "{labels:?}");
    assert!(!labels.contains(&"double".to_string()), "{labels:?}");

    // A call-chain receiver: methods of the constructed value's type.
    let labels = complete_after("m.Point.new():");
    assert!(labels.contains(&"shift".to_string()), "{labels:?}");
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
export type State = { middle: {Card}, round: i32 }\n\
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
fn readonly_aliases_preserve_record_member_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "type View = readonly<{ count: i32 }>\n\
local view: View = { count = 1::i32 }\n\
view.\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);
    let result = request(&mut server, "textDocument/completion", &path, 2, 5);
    let labels: Vec<&str> = result
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"count"), "{labels:?}");
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

/// A marked-up fixture file: `<|>` is the cursor and `<def>`/`</def>` bracket
/// the identifier the request should land on. Markers are stripped before the
/// file is written.
struct Marked {
    name: &'static str,
    text: String,
    cursor: Option<u32>,
    expected: Option<(u32, u32)>,
}

fn strip_markers(name: &'static str, raw: &str) -> Marked {
    let mut text = String::new();
    let mut cursor = None;
    let (mut start, mut end) = (None, None);
    let mut rest = raw;
    while let Some((index, marker)) = ["<|>", "<def>", "</def>"]
        .iter()
        .filter_map(|marker| rest.find(marker).map(|index| (index, *marker)))
        .min_by_key(|(index, _)| *index)
    {
        text.push_str(&rest[..index]);
        let at = text.chars().count() as u32;
        match marker {
            "<|>" => cursor = Some(at),
            "<def>" => start = Some(at),
            _ => end = Some(at),
        }
        rest = &rest[index + marker.len()..];
    }
    text.push_str(rest);
    Marked {
        name,
        text,
        cursor,
        expected: start.zip(end),
    }
}

/// LSP position of a character offset, matching the server's own conversion.
fn lsp_position(text: &str, offset: u32) -> (u32, u32) {
    let mut line = 0u32;
    let mut character = 0u32;
    for ch in text.chars().take(offset as usize) {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }
    (line, character)
}

/// Run one marked-up go-to-definition case end to end through the server.
/// Returns a failure description, or `None` when the jump landed exactly on
/// the `<def>` marker.
fn definition_case(case: &str, files: &[(&'static str, &str)]) -> Option<String> {
    let dir = tempfile::tempdir().expect("tempdir");
    let marked: Vec<Marked> = files
        .iter()
        .map(|(name, raw)| strip_markers(name, raw))
        .collect();
    for file in &marked {
        let path = dir.path().join(file.name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
        std::fs::write(&path, &file.text).expect("write fixture");
    }
    let query = marked.iter().find(|file| file.cursor.is_some())?;
    let target = marked.iter().find(|file| file.expected.is_some())?;
    let (expected_start, expected_end) = target.expected.expect("target marker");

    let mut server = LspServer::new();
    let query_path = dir.path().join(query.name);
    open(&mut server, &query_path, &query.text);
    let (line, character) = lsp_position(&query.text, query.cursor.expect("cursor"));
    let location = request(
        &mut server,
        "textDocument/definition",
        &query_path,
        line,
        character,
    );

    let want_start = lsp_position(&target.text, expected_start);
    let want_end = lsp_position(&target.text, expected_end);
    let Some(uri) = location["uri"].as_str() else {
        return Some(format!("{case}: no definition"));
    };
    let got = (
        location["range"]["start"]["line"].as_u64().unwrap_or(0) as u32,
        location["range"]["start"]["character"]
            .as_u64()
            .unwrap_or(0) as u32,
    );
    let got_end = (
        location["range"]["end"]["line"].as_u64().unwrap_or(0) as u32,
        location["range"]["end"]["character"].as_u64().unwrap_or(0) as u32,
    );
    if !uri.ends_with(target.name) {
        return Some(format!("{case}: landed in {uri}, want {}", target.name));
    }
    (got != want_start || got_end != want_end)
        .then(|| format!("{case}: got {got:?}..{got_end:?}, want {want_start:?}..{want_end:?}"))
}

/// Go-to-definition across the shapes a function or method reference takes.
/// Every case is one cursor and one expected target; the table is the point,
/// so a regression names the shape it broke.
#[test]
fn definition_resolves_functions_and_methods_through_every_receiver_shape() {
    let cases: Vec<(&str, Vec<(&'static str, &str)>)> = vec![
        (
            "top-level function call",
            vec![(
                "main.walu",
                "function <def>add</def>(a: i32): i32\n    return a\nend\nlocal x = <|>add(1)\n",
            )],
        ),
        (
            "local function call",
            vec![(
                "main.walu",
                "local function <def>helper</def>(): i32\n    return 1\nend\nlocal x = <|>helper()\n",
            )],
        ),
        (
            "forward reference to a later function",
            vec![(
                "main.walu",
                "function first(): i32\n    return <|>later()\nend\nfunction <def>later</def>(): i32\n    return 1\nend\n",
            )],
        ),
        (
            "dot-named function",
            vec![(
                "main.walu",
                "function <def>State.new</def>(): i32\n    return 1\nend\nlocal s = State.<|>new()\n",
            )],
        ),
        (
            "method on an annotated local",
            vec![(
                "main.walu",
                "type P = { x: i32 }\nfunction <def>P:get</def>(): i32\n    return self.x\nend\nlocal p: P = { x = 1 }\nlocal v = p:<|>get()\n",
            )],
        ),
        (
            "method on a local inferred from a constructor",
            vec![(
                "main.walu",
                "type P = { x: i32 }\nfunction P.new(): P\n    return { x = 1 }\nend\nfunction <def>P:get</def>(): i32\n    return self.x\nend\nlocal p = P.new()\nlocal v = p:<|>get()\n",
            )],
        ),
        (
            "method on self inside another method",
            vec![(
                "main.walu",
                "type P = { x: i32 }\nfunction <def>P:helper</def>(): i32\n    return self.x\nend\nfunction P:outer(): i32\n    return self:<|>helper()\nend\n",
            )],
        ),
        (
            "chained call receiver",
            vec![(
                "main.walu",
                "type P = { x: i32 }\nfunction P.new(): P\n    return { x = 1 }\nend\nfunction <def>P:get</def>(): i32\n    return self.x\nend\nlocal v = P.new():<|>get()\n",
            )],
        ),
        (
            "method on an indexed element",
            vec![(
                "main.walu",
                "type Q = { y: i32 }\nfunction <def>Q:get</def>(): i32\n    return self.y\nend\nlocal items: [Q] = {}\nlocal v = items[1]:<|>get()\n",
            )],
        ),
        (
            "method on a record field",
            vec![(
                "main.walu",
                "type Q = { y: i32 }\ntype P = { q: Q }\nfunction <def>Q:get</def>(): i32\n    return self.y\nend\nlocal p: P = { q = { y = 1 } }\nlocal v = p.q:<|>get()\n",
            )],
        ),
        (
            "method on a local from a method call",
            vec![(
                "main.walu",
                "type P = { x: i32 }\ntype Q = { y: i32 }\nfunction P:to_q(): Q\n    return { y = 1 }\nend\nfunction <def>Q:get</def>(): i32\n    return self.y\nend\nlocal p: P = { x = 1 }\nlocal q = p:to_q()\nlocal v = q:<|>get()\n",
            )],
        ),
        (
            "same method name on two types",
            vec![(
                "main.walu",
                "type A = { x: i32 }\ntype B = { x: i32 }\nfunction A:get(): i32\n    return self.x\nend\nfunction <def>B:get</def>(): i32\n    return self.x\nend\nlocal b: B = { x = 1 }\nlocal v = b:<|>get()\n",
            )],
        ),
        (
            "cursor at the end of the identifier",
            vec![(
                "main.walu",
                "function <def>add</def>(a: i32): i32\n    return a\nend\nlocal x = add<|>(1)\n",
            )],
        ),
        (
            "function referenced as a value",
            vec![(
                "main.walu",
                "function <def>add</def>(a: i32): i32\n    return a\nend\nlocal f = <|>add\n",
            )],
        ),
        (
            "reference before a syntax error later in the file",
            vec![(
                "main.walu",
                "function <def>add</def>(a: i32): i32\n    return a\nend\nlocal x = <|>add(1)\nlocal y = = =\n",
            )],
        ),
        (
            "cross-module function through a require alias",
            vec![
                (
                    "lib.walu",
                    "function <def>double</def>(x: i32): i32\n    return x * 2\nend\n",
                ),
                (
                    "main.walu",
                    "local m = require(\"./lib\")\nlocal y = m.<|>double(4)\n",
                ),
            ],
        ),
        (
            "cross-module dot-named function through a require alias",
            vec![
                (
                    "lib.walu",
                    "export type P = { x: i32 }\nfunction <def>P.new</def>(): P\n    return { x = 1 }\nend\n",
                ),
                (
                    "main.walu",
                    "local m = require(\"./lib\")\nlocal p = m.P.<|>new()\n",
                ),
            ],
        ),
        (
            "method on a value built through a module-qualified constructor",
            vec![
                (
                    "lib.walu",
                    "export type P = { x: i32 }\nfunction P.new(): P\n    return { x = 1 }\nend\nfunction <def>P:get</def>(): i32\n    return self.x\nend\n",
                ),
                (
                    "main.walu",
                    "local m = require(\"./lib\")\nlocal p = m.P.new()\nlocal v = p:<|>get()\n",
                ),
            ],
        ),
        (
            "method on a module-qualified annotation",
            vec![
                (
                    "lib.walu",
                    "export type P = { x: i32 }\nfunction <def>P:get</def>(): i32\n    return self.x\nend\n",
                ),
                (
                    "main.walu",
                    "local m = require(\"./lib\")\nfunction use(p: m.P): i32\n    return p:<|>get()\nend\n",
                ),
            ],
        ),
        (
            "same function name in two modules",
            vec![
                ("a.walu", "function shared(): i32\n    return 1\nend\n"),
                (
                    "b.walu",
                    "function <def>shared</def>(): i32\n    return 2\nend\n",
                ),
                (
                    "main.walu",
                    "local a = require(\"./a\")\nlocal b = require(\"./b\")\nlocal v = b.<|>shared()\n",
                ),
            ],
        ),
        (
            "require from a subdirectory",
            vec![
                (
                    "sub/lib.walu",
                    "function <def>double</def>(x: i32): i32\n    return x * 2\nend\n",
                ),
                (
                    "main.walu",
                    "local m = require(\"./sub/lib\")\nlocal v = m.<|>double(4)\n",
                ),
            ],
        ),
    ];

    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(name, files)| definition_case(name, files))
        .collect();
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

/// Compiler spans count characters, but editors send UTF-16 columns and Rust
/// slices bytes. A non-ASCII character anywhere earlier in the file used to
/// shift every later jump; this pins all three units together.
#[test]
fn positions_survive_non_ascii_text_earlier_in_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    // Three em-dashes: two extra bytes each over their character count.
    let text = "-- a note — with an em-dash — and another —\n\
function double(x: i32): i32\n\
    return x * 2\n\
end\n\
local y: i32 = double(4)\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    let (line, character) = position_of(text, "double(4)");
    let location = request(
        &mut server,
        "textDocument/definition",
        &path,
        line,
        character,
    );
    // `function double` is on line 1; the name starts at column 9.
    assert_eq!(location["range"]["start"]["line"], json!(1), "{location:?}");
    assert_eq!(
        location["range"]["start"]["character"],
        json!(9),
        "{location:?}"
    );
    assert_eq!(
        location["range"]["end"]["character"],
        json!(15),
        "{location:?}"
    );

    // Hover resolves from the same position, proving the inbound conversion
    // agrees with the outbound one.
    let hover = request(&mut server, "textDocument/hover", &path, line, character);
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(
        contents.contains("function double(x: i32): i32"),
        "{contents}"
    );
}

fn references_request(
    server: &mut LspServer,
    path: &Path,
    line: u32,
    character: u32,
    include_declaration: bool,
) -> Vec<(String, u32, u32)> {
    let responses = send(
        server,
        json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "textDocument/references",
            "params": {
                "textDocument": {"uri": format!("file://{}", path.display())},
                "position": {"line": line, "character": character},
                "context": {"includeDeclaration": include_declaration},
            },
        }),
    );
    responses[0]["result"]
        .as_array()
        .expect("locations")
        .iter()
        .map(|location| {
            let uri = location["uri"].as_str().expect("uri");
            (
                uri.rsplit('/').next().expect("file name").to_string(),
                location["range"]["start"]["line"].as_u64().expect("line") as u32,
                location["range"]["start"]["character"]
                    .as_u64()
                    .expect("character") as u32,
            )
        })
        .collect()
}

/// A function passed as a value — not called — is still a reference.
#[test]
fn references_find_a_function_used_as_a_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    let text = "local function draw_drawing(alpha: f64): unit\n\
end\n\
local story = { draw = draw_drawing }\n\
local again = draw_drawing\n";
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    let (line, character) = position_of(text, "draw_drawing }");
    let found = references_request(&mut server, &path, line, character, true);
    assert_eq!(
        found,
        vec![
            ("main.walu".to_string(), 0, 15),
            ("main.walu".to_string(), 2, 23),
            ("main.walu".to_string(), 3, 14),
        ],
        "declaration plus both value uses"
    );

    let without = references_request(&mut server, &path, line, character, false);
    assert_eq!(
        without,
        vec![
            ("main.walu".to_string(), 2, 23),
            ("main.walu".to_string(), 3, 14),
        ],
        "declaration excluded on request"
    );
}

/// References resolve each occurrence rather than matching text, so a
/// shadowing inner binding does not absorb the outer one's uses.
#[test]
fn references_separate_shadowed_bindings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.walu");
    // Written without line continuations: the indentation is load-bearing,
    // and `\` at a line end would strip it.
    let text = concat!(
        "local function run(): i32\n",
        "    return 1\n",
        "end\n",
        "local outer = run()\n",
        "do\n",
        "    local function run(): i32\n",
        "        return 2\n",
        "    end\n",
        "    local inner = run()\n",
        "end\n",
        "local after = run()\n",
    );
    std::fs::write(&path, text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &path, text);

    // The outer binding: its own declaration and the two uses outside the block.
    let (line, character) = position_of(text, "run()");
    let found = references_request(&mut server, &path, line, character, true);
    assert_eq!(
        found,
        vec![
            ("main.walu".to_string(), 0, 15),
            ("main.walu".to_string(), 3, 14),
            ("main.walu".to_string(), 10, 14),
        ],
        "outer `run` must not absorb the shadowed use"
    );

    // The inner binding: only the shadowed declaration and its single use.
    let (line, character) = position_of(text, "run(): i32\n        return 2");
    let found = references_request(&mut server, &path, line, character, true);
    assert_eq!(
        found,
        vec![
            ("main.walu".to_string(), 5, 19),
            ("main.walu".to_string(), 8, 18),
        ],
        "inner `run` sees only its own scope"
    );
}

/// A method's uses are found through the receiver's type, and a same-named
/// method on another type is not swept in.
#[test]
fn references_follow_methods_and_cross_module_uses() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = dir.path().join("lib.walu");
    let lib_text = "export type P = { x: i32 }\n\
type Other = { x: i32 }\n\
function P.new(): P\n\
    return { x = 1 }\n\
end\n\
function P:get(): i32\n\
    return self.x\n\
end\n\
function Other:get(): i32\n\
    return self.x\n\
end\n";
    std::fs::write(&lib, lib_text).expect("write fixture");
    let main = dir.path().join("main.walu");
    let main_text = "local m = require(\"./lib\")\n\
local p = m.P.new()\n\
local a: i32 = p:get()\n\
local b: i32 = p:get()\n";
    std::fs::write(&main, main_text).expect("write fixture");

    let mut server = LspServer::new();
    open(&mut server, &main, main_text);
    // The workspace walk needs a root; `initialize` supplies it.
    send(
        &mut server,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"rootUri": format!("file://{}", dir.path().display())},
        }),
    );

    let (line, character) = position_of(main_text, "get()");
    let found = references_request(&mut server, &main, line, character, true);
    assert_eq!(
        found,
        vec![
            // `function P:get` in the module, then both call sites.
            ("lib.walu".to_string(), 5, 11),
            ("main.walu".to_string(), 2, 17),
            ("main.walu".to_string(), 3, 17),
        ],
        "`Other:get` must not be swept in"
    );
}
