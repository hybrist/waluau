//! Parses every `.walu` file in the repository corpus and asserts the CST
//! parser accepts all of them. A powerful regression oracle: these files all
//! compile, so the formatter must at least parse them.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <root>/crates/waluau-fmt
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn walu_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walu_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "walu") {
            out.push(path);
        }
    }
}

fn corpus() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut files = Vec::new();
    walu_files(&root.join("conformance"), &mut files);
    walu_files(&root.join("fixtures"), &mut files);
    files.sort();
    files
}

/// The formatter only needs to handle source the compiler accepts. Files the
/// real `waluau-parser` rejects (e.g. the vendored upstream-Luau suite using
/// syntax Waluau does not support) are out of scope.
fn compiler_accepts(src: &str) -> bool {
    waluau_parser::parse(src).is_ok()
}

#[test]
fn parses_entire_corpus() {
    let files = corpus();
    assert!(!files.is_empty(), "no corpus files found");

    let mut failures = Vec::new();
    let mut total = 0usize;
    for path in &files {
        let src = std::fs::read_to_string(path).expect("read file");
        if !compiler_accepts(&src) {
            continue;
        }
        total += 1;
        if let Err(err) = waluau_fmt::parse(&src) {
            failures.push(format!("{}: {}", path.display(), err.render()));
        }
    }

    let failed = failures.len();
    if failed > 0 {
        let shown: Vec<_> = failures.iter().take(25).cloned().collect();
        panic!(
            "{failed}/{total} corpus files failed to parse:\n{}",
            shown.join("\n")
        );
    }
    eprintln!("parsed {total} compiler-accepted corpus files");
    // Guard against the filter silently excluding everything.
    assert!(total > 500, "expected a large in-scope corpus, got {total}");
}
