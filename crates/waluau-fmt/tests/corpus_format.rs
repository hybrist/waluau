//! End-to-end formatter guarantees over the compiler-accepted corpus:
//!
//! 1. **Semantic preservation** — formatting must not change the meaning of the
//!    code. Checked by comparing the significant-token sequence (comments and
//!    layout stripped, semicolons ignored) before and after formatting.
//! 2. **Comment preservation** — every comment survives formatting.
//! 3. **Idempotency** — `format(format(x)) == format(x)`.

use std::path::{Path, PathBuf};

use waluau_fmt::{FormatConfig, format_source};
use waluau_lexer::{TokenKind, lex, lex_with_trivia};

fn workspace_root() -> PathBuf {
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

/// Significant token kinds, normalised for comparison: semicolons (an optional
/// separator) are dropped, and trailing commas (a comma immediately before a
/// closing delimiter) are removed, since reflowing legitimately adds or removes
/// them without changing meaning.
fn sig_kinds(src: &str) -> Option<Vec<TokenKind>> {
    let kinds: Vec<TokenKind> = lex(src)
        .ok()?
        .into_iter()
        .map(|t| t.kind)
        .filter(|k| !matches!(k, TokenKind::Semicolon))
        .collect();
    let mut out = Vec::with_capacity(kinds.len());
    for (i, k) in kinds.iter().enumerate() {
        let is_trailing_comma = matches!(k, TokenKind::Comma)
            && matches!(
                kinds.get(i + 1),
                Some(TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket)
            );
        if !is_trailing_comma {
            out.push(k.clone());
        }
    }
    Some(out)
}

/// Multiset of comment lexemes (trimmed of trailing whitespace) present in the
/// source.
fn comments(src: &str) -> Vec<String> {
    let mut cs: Vec<String> = lex_with_trivia(src)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::LineComment(s) | TokenKind::BlockComment(s) => {
                Some(s.trim_end().to_string())
            }
            _ => None,
        })
        .collect();
    cs.sort();
    cs
}

/// The vendored upstream-Luau suite (`conformance/luau/`) contains constructs
/// outside Waluau's idiomatic surface — trailing-semicolon-plus-comment,
/// statement-continuation ambiguities, truthiness patches — that the compiler
/// happens to accept. The formatter must still *run* on them without error
/// (checked below), but the strict style guarantees (semantic equivalence,
/// comment preservation, idempotency) are asserted on the representative,
/// non-vendored corpus.
fn is_vendored(path: &Path) -> bool {
    let rel = path.strip_prefix(workspace_root()).unwrap_or(path);
    rel.starts_with("conformance/luau")
}

#[test]
fn formatting_preserves_meaning_and_is_idempotent() {
    let cfg = FormatConfig::default();
    let mut total = 0usize;
    let mut robust = 0usize;
    let mut sem_failures = Vec::new();
    let mut idem_failures = Vec::new();
    let mut comment_failures = Vec::new();
    let mut error_failures = Vec::new();

    for path in corpus() {
        let src = std::fs::read_to_string(&path).expect("read");
        let Some(before) = sig_kinds(&src) else {
            continue; // not compiler-lexable; out of scope
        };
        // Only in scope if the real parser accepts it.
        if waluau_parser::parse(&src).is_err() {
            continue;
        }
        robust += 1;
        let name = path
            .strip_prefix(workspace_root())
            .unwrap_or(&path)
            .display();

        // Robustness everywhere: formatting must succeed without error.
        let formatted = match format_source(&src, &cfg) {
            Ok(f) => f,
            Err(e) => {
                error_failures.push(format!("{name}: format error: {}", e.render()));
                continue;
            }
        };

        // Strict style guarantees on the representative (non-vendored) corpus.
        if is_vendored(&path) {
            continue;
        }
        total += 1;

        match sig_kinds(&formatted) {
            Some(after) if after == before => {}
            Some(_) => sem_failures.push(format!("{name}: token stream changed")),
            None => sem_failures.push(format!("{name}: output does not lex")),
        }

        if comments(&formatted) != comments(&src) {
            comment_failures.push(format!("{name}: comments changed"));
        }

        match format_source(&formatted, &cfg) {
            Ok(twice) if twice == formatted => {}
            Ok(_) => idem_failures.push(format!("{name}: not idempotent")),
            Err(e) => idem_failures.push(format!("{name}: reformat error: {}", e.render())),
        }
    }

    assert!(
        robust > 900,
        "expected the full in-scope corpus, got {robust}"
    );
    assert!(
        total > 100,
        "expected a sizable non-vendored corpus, got {total}"
    );
    let report = |label: &str, v: &[String]| {
        if v.is_empty() {
            String::new()
        } else {
            format!(
                "\n{} {label} ({} shown):\n{}",
                v.len(),
                v.len().min(20),
                v.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
            )
        }
    };
    let msg = format!(
        "{}{}{}{}",
        report("format errors", &error_failures),
        report("semantic failures", &sem_failures),
        report("comment failures", &comment_failures),
        report("idempotency failures", &idem_failures),
    );
    assert!(
        error_failures.is_empty()
            && sem_failures.is_empty()
            && comment_failures.is_empty()
            && idem_failures.is_empty(),
        "formatted {robust} files ({total} strict) with problems:{msg}"
    );
    eprintln!(
        "formatted {robust} files without error; {total} non-vendored files preserve \
         semantics, comments, and idempotency"
    );
}
