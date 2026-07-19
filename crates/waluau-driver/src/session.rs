//! Stateful compiler session for incremental analysis.
//!
//! The session is a shared file store — in-memory overlays (open editor
//! buffers) layered over the filesystem — plus a content-hash-keyed parse
//! cache. It is deliberately *root-agnostic*: any file can be analyzed as its
//! own root, pulling in its own `require` subgraph, so files not (yet)
//! imported from a main entry point — a freshly created file, a test file
//! requiring production modules — are first-class. Parses are shared across
//! roots and across repeated analyses; only files whose content changed are
//! re-parsed.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use waluau_diagnostics::Diagnostic;

use crate::link::{self, ModuleProvider};

#[derive(Default)]
pub struct CompilerSession {
    /// In-memory file contents that take precedence over the filesystem.
    overlays: HashMap<PathBuf, String>,
    parse_cache: HashMap<PathBuf, CachedParse>,
    parses_performed: usize,
}

struct CachedParse {
    content_hash: u64,
    program: waluau_ast::Program,
    diagnostics: Vec<Diagnostic>,
}

/// Result of analyzing one root: every diagnostic attributable to the root's
/// module graph, and the graph's source files (for watch invalidation).
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    pub involved_files: Vec<PathBuf>,
}

/// Result of building one root through the session. `artifacts` is present
/// exactly when `diagnostics` contains no errors.
pub struct BuildOutcome {
    pub artifacts: Option<crate::CompileArtifacts>,
    pub diagnostics: Vec<Diagnostic>,
    pub involved_files: Vec<PathBuf>,
}

impl CompilerSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or update) an in-memory overlay for `path`, e.g. an open editor
    /// buffer. Later analyses see this content instead of the file on disk.
    pub fn set_overlay(&mut self, path: impl Into<PathBuf>, content: impl Into<String>) {
        self.overlays.insert(path.into(), content.into());
    }

    /// Drop the overlay for `path`, falling back to the filesystem.
    pub fn remove_overlay(&mut self, path: &Path) {
        self.overlays.remove(path);
    }

    /// Analyze `root` and its `require` subgraph: all parse diagnostics with
    /// recovery, then (when parsing is clean) all type-checker diagnostics.
    pub fn analyze_root(&mut self, root: &Path) -> Analysis {
        let outcome = match link::link_program_collect_with(root, &mut provider(self)) {
            Ok(outcome) => outcome,
            Err(error) => {
                return Analysis {
                    diagnostics: vec![error],
                    involved_files: Vec::new(),
                };
            }
        };
        if !outcome.diagnostics.is_empty() {
            return Analysis {
                diagnostics: outcome.diagnostics,
                involved_files: outcome.involved_files,
            };
        }
        let diagnostics = match waluau_hir::type_check_and_infer_collect(&outcome.program) {
            Ok(_) => Vec::new(),
            Err(errors) => errors
                .into_iter()
                .map(|error| crate::resolve_diagnostic_source(error, &outcome.program))
                .collect(),
        };
        Analysis {
            diagnostics,
            involved_files: outcome.involved_files,
        }
    }

    /// Compile `root` to artifacts through the session's caches, reporting
    /// all diagnostics and the involved files even when the build fails (so
    /// watch mode can still register the whole graph).
    pub fn build_root(&mut self, root: &Path, wasm_file_name: &str) -> BuildOutcome {
        let outcome = match link::link_program_collect_with(root, &mut provider(self)) {
            Ok(outcome) => outcome,
            Err(error) => {
                return BuildOutcome {
                    artifacts: None,
                    diagnostics: vec![error],
                    involved_files: Vec::new(),
                };
            }
        };
        if !outcome.diagnostics.is_empty() {
            return BuildOutcome {
                artifacts: None,
                diagnostics: outcome.diagnostics,
                involved_files: outcome.involved_files,
            };
        }
        match crate::compile_program(
            outcome.program,
            wasm_file_name,
            crate::empty_asset_manifest(),
        ) {
            Ok(artifacts) => BuildOutcome {
                artifacts: Some(artifacts),
                diagnostics: Vec::new(),
                involved_files: outcome.involved_files,
            },
            Err(diagnostics) => BuildOutcome {
                artifacts: None,
                diagnostics,
                involved_files: outcome.involved_files,
            },
        }
    }

    fn read(&self, path: &Path) -> Result<String, Diagnostic> {
        if let Some(content) = self.overlays.get(path) {
            return Ok(content.clone());
        }
        std::fs::read_to_string(path).map_err(|error| {
            Diagnostic::new(format!("read module `{}`: {error}", path.display()))
        })
    }

    fn parsed_module_cached(
        &mut self,
        path: &Path,
    ) -> Result<(waluau_ast::Program, Vec<Diagnostic>), Diagnostic> {
        let content = self.read(path)?;
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        let content_hash = hasher.finish();

        if let Some(cached) = self.parse_cache.get(path)
            && cached.content_hash == content_hash
        {
            return Ok((cached.program.clone(), cached.diagnostics.clone()));
        }

        let outcome = waluau_parser::parse_with_recovery(&content, &path.to_string_lossy());
        self.parses_performed += 1;
        self.parse_cache.insert(
            path.to_path_buf(),
            CachedParse {
                content_hash,
                program: outcome.program.clone(),
                diagnostics: outcome.diagnostics.clone(),
            },
        );
        Ok((outcome.program, outcome.diagnostics))
    }

    /// Number of files with a live cached parse (test/diagnostic aid).
    pub fn cached_parse_count(&self) -> usize {
        self.parse_cache.len()
    }

    /// Total parses performed over the session's lifetime; a cache hit does
    /// not increment this (test/diagnostic aid).
    pub fn parses_performed(&self) -> usize {
        self.parses_performed
    }
}

/// Adapter lending the session to the linker as a [`ModuleProvider`].
fn provider(session: &mut CompilerSession) -> SessionModules<'_> {
    SessionModules { session }
}

struct SessionModules<'a> {
    session: &'a mut CompilerSession,
}

impl ModuleProvider for SessionModules<'_> {
    fn parsed_module(
        &mut self,
        path: &Path,
    ) -> Result<(waluau_ast::Program, Vec<Diagnostic>), Diagnostic> {
        self.session.parsed_module_cached(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).expect("fixture should write");
        path
    }

    #[test]
    fn shared_modules_parse_once_across_overlapping_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "lib.walu",
            "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n",
        );
        let main = write(
            dir.path(),
            "main.walu",
            "local double = require(\"./lib\")\nlocal a: i32 = double(2)\n",
        );
        let test_root = write(
            dir.path(),
            "lib.test.walu",
            "local double = require(\"./lib\")\nlocal b: i32 = double(4)\n",
        );

        let mut session = CompilerSession::new();
        let first = session.analyze_root(&main);
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        assert_eq!(first.involved_files.len(), 2);
        let parses_after_first = session.parses_performed();

        // The test file is its own root — not reachable from main — and its
        // subgraph shares lib.walu, which must come from the cache.
        let second = session.analyze_root(&test_root);
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.involved_files.len(), 2);
        assert_eq!(
            session.parses_performed(),
            parses_after_first + 1,
            "only the new root should be parsed"
        );

        // Re-analyzing with nothing changed re-parses nothing.
        session.analyze_root(&main);
        assert_eq!(session.parses_performed(), parses_after_first + 1);
    }

    #[test]
    fn overlay_changes_invalidate_only_that_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "lib.walu",
            "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n",
        );
        let main = write(
            dir.path(),
            "main.walu",
            "local double = require(\"./lib\")\nlocal a: i32 = double(2)\n",
        );

        let mut session = CompilerSession::new();
        session.analyze_root(&main);
        let baseline = session.parses_performed();

        // Simulate an editor keystroke in main.walu via an overlay. Note the
        // overlay key must match the canonical path the linker resolves.
        let canonical_main = main.canonicalize().expect("canonicalize");
        session.set_overlay(
            &canonical_main,
            "local double = require(\"./lib\")\nlocal a: i32 = double(3)\n",
        );
        let analysis = session.analyze_root(&main);
        assert!(analysis.diagnostics.is_empty(), "{:?}", analysis.diagnostics);
        assert_eq!(
            session.parses_performed(),
            baseline + 1,
            "only the overlaid file should re-parse"
        );
    }

    #[test]
    fn analyze_reports_type_errors_with_module_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = write(
            dir.path(),
            "main.walu",
            "function first(x: i32): bool\n    return x\nend\nfunction second(x: i32): i32\n    if x then\n        return x\n    end\n    return x\nend\n",
        );

        let mut session = CompilerSession::new();
        let analysis = session.analyze_root(&main);
        assert_eq!(analysis.diagnostics.len(), 2, "{:?}", analysis.diagnostics);
        for diagnostic in &analysis.diagnostics {
            assert!(
                diagnostic.file_path().is_some_and(|p| p.contains("main.walu")),
                "diagnostic should carry the module path: {diagnostic:?}"
            );
        }
    }

    #[test]
    fn build_root_produces_artifacts_and_involved_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "lib.walu",
            "function double(x: i32): i32\n    return x * 2\nend\nreturn double\n",
        );
        let main = write(
            dir.path(),
            "main.walu",
            "local double = require(\"./lib\")\nfunction entry(): i32\n    return double(21)\nend\n",
        );

        let mut session = CompilerSession::new();
        let outcome = session.build_root(&main, "program.wasm");
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
        let artifacts = outcome.artifacts.expect("artifacts should be produced");
        assert!(!artifacts.wasm.is_empty());
        assert_eq!(outcome.involved_files.len(), 2);
    }
}
