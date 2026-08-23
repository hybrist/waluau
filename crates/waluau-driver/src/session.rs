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
    hir_caches: HashMap<PathBuf, waluau_hir::TypeCheckCache>,
    ir_caches: HashMap<PathBuf, waluau_ir::BuildCache>,
    wasm_caches: HashMap<PathBuf, waluau_codegen_wasm::EmitCache>,
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
        let asset_module_source = match crate::discover_asset_module(root) {
            Ok(source) => source,
            Err(error) => {
                return Analysis {
                    diagnostics: vec![error],
                    involved_files: Vec::new(),
                };
            }
        };
        let outcome = match link::link_program_collect_with_assets(
            root,
            &mut provider(self),
            asset_module_source.as_deref(),
        ) {
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
        self.build_root_with_options(root, wasm_file_name, crate::CompileOptions::default())
    }

    pub fn build_root_with_options(
        &mut self,
        root: &Path,
        wasm_file_name: &str,
        options: crate::CompileOptions,
    ) -> BuildOutcome {
        self.build_root_with_assets(
            root,
            wasm_file_name,
            crate::empty_asset_manifest(),
            None,
            options,
        )
    }

    pub(crate) fn build_root_with_assets(
        &mut self,
        root: &Path,
        wasm_file_name: &str,
        assets: &std::collections::BTreeMap<String, waluau_codegen_wasm::GeneratedAsset>,
        asset_module_source: Option<&str>,
        options: crate::CompileOptions,
    ) -> BuildOutcome {
        let discovered_asset_module;
        let asset_module_source = match asset_module_source {
            Some(source) => Some(source),
            None => {
                discovered_asset_module = match crate::discover_asset_module(root) {
                    Ok(source) => source,
                    Err(error) => {
                        return BuildOutcome {
                            artifacts: None,
                            diagnostics: vec![error],
                            involved_files: Vec::new(),
                        };
                    }
                };
                discovered_asset_module.as_deref()
            }
        };
        let outcome = match link::link_program_collect_with_assets(
            root,
            &mut provider(self),
            asset_module_source,
        ) {
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
        let cache_key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let hir_cache = self.hir_caches.entry(cache_key.clone()).or_default();
        let ir_cache = self.ir_caches.entry(cache_key.clone()).or_default();
        let wasm_cache = self.wasm_caches.entry(cache_key).or_default();
        match crate::compile_program_with_cache(
            outcome.program,
            wasm_file_name,
            assets,
            options,
            Some(hir_cache),
            Some(ir_cache),
            Some(wasm_cache),
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
        std::fs::read_to_string(path)
            .map_err(|error| Diagnostic::new(format!("read module `{}`: {error}", path.display())))
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

    /// Incremental phase state from the most recent successful build.
    pub fn incremental_stats(&self, root: &Path) -> (usize, bool, bool) {
        let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        (
            self.hir_caches
                .get(&key)
                .map_or(0, waluau_hir::TypeCheckCache::reused_function_count),
            self.ir_caches
                .get(&key)
                .is_some_and(waluau_ir::BuildCache::last_build_was_incremental),
            self.wasm_caches
                .get(&key)
                .is_some_and(waluau_codegen_wasm::EmitCache::last_emit_was_incremental),
        )
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
        assert!(
            analysis.diagnostics.is_empty(),
            "{:?}",
            analysis.diagnostics
        );
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
                diagnostic
                    .file_path()
                    .is_some_and(|p| p.contains("main.walu")),
                "diagnostic should carry the module path: {diagnostic:?}"
            );
        }
    }

    #[test]
    fn analysis_discovers_the_manifest_generated_asset_module() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "waluau.assets.json",
            r#"{"version":1,"assets":[{"name":"card","path":"assets/card.png","type":"image"}]}"#,
        );
        let main = write(
            dir.path(),
            "main.walu",
            "local assets = require(\"waluau:assets\")\nfunction load(): assets.LoadResult\n    return assets.load()\nend\n",
        );

        let mut session = CompilerSession::new();
        let analysis = session.analyze_root(&main);
        assert!(
            analysis.diagnostics.is_empty(),
            "{:?}",
            analysis.diagnostics
        );
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

    #[test]
    fn numeric_body_edit_reuses_hir_ir_and_wasm_products() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = write(
            dir.path(),
            "main.walu",
            "function unchanged(): i32\n    return 7\nend\nfunction changed(): i32\n    return 32\nend\n",
        );
        let changed_source = "function unchanged(): i32\n    return 7\nend\nfunction changed(): i32\n    return 33\nend\n";

        let mut session = CompilerSession::new();
        let first = session.build_root(&main, "program.wasm");
        let first_wasm = first.artifacts.expect("cold build artifacts").wasm;
        let canonical_main = main.canonicalize().expect("canonicalize");
        session.set_overlay(&canonical_main, changed_source);
        let changed = session.build_root(&main, "program.wasm");
        assert!(changed.diagnostics.is_empty(), "{:?}", changed.diagnostics);
        let changed_wasm = changed.artifacts.expect("incremental artifacts").wasm;
        assert_ne!(
            first_wasm, changed_wasm,
            "the edited literal must change Wasm"
        );
        let (reused_hir, incremental_ir, incremental_wasm) = session.incremental_stats(&main);
        assert!(
            reused_hir >= 1,
            "an unchanged function should reuse typed HIR"
        );
        assert!(incremental_ir, "IR should lower only the changed function");
        assert!(incremental_wasm, "Wasm should patch only the changed body");

        let cold = write(dir.path(), "cold.walu", changed_source);
        let cold_wasm = CompilerSession::new()
            .build_root(&cold, "program.wasm")
            .artifacts
            .expect("cold comparison artifacts")
            .wasm;
        assert_eq!(
            wasmprinter::print_bytes(&changed_wasm).expect("incremental Wasm should print"),
            wasmprinter::print_bytes(&cold_wasm).expect("cold Wasm should print"),
            "incremental and cold builds should be semantically identical"
        );
    }

    #[test]
    fn emit_option_changes_invalidate_the_incremental_wasm_cache() {
        fn has_export(wasm: &[u8], name: &str) -> bool {
            wasmprinter::print_bytes(wasm)
                .expect("wasm should print")
                .contains(&format!("(export \"{name}\""))
        }
        fn build(session: &mut CompilerSession, root: &Path, minimal: bool) -> Vec<u8> {
            let outcome = session.build_root_with_options(
                root,
                "program.wasm",
                crate::CompileOptions {
                    minimal_exports: minimal,
                    ..Default::default()
                },
            );
            assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
            outcome.artifacts.expect("artifacts").wasm
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let main = write(
            dir.path(),
            "main.walu",
            "function helper(x: i32): i32\n    return x + 1\nend\nassert(helper(1) >= 0)\n",
        );
        let canonical_main = main.canonicalize().expect("canonicalize");

        let mut session = CompilerSession::new();
        assert!(has_export(&build(&mut session, &main, false), "helper"));

        // A single-body edit is exactly the shape the incremental emitter
        // patches; flipping the options at the same time must not let it
        // reuse the cached (full-export) section image.
        session.set_overlay(
            &canonical_main,
            "function helper(x: i32): i32\n    return x + 2\nend\nassert(helper(1) >= 0)\n",
        );
        let minimal = build(&mut session, &main, true);
        assert!(!has_export(&minimal, "helper"));
        assert!(has_export(&minimal, "main"));

        // With options stable again, the next single-body edit may patch
        // incrementally — and must stay minimal.
        session.set_overlay(
            &canonical_main,
            "function helper(x: i32): i32\n    return x + 3\nend\nassert(helper(1) >= 0)\n",
        );
        let patched = build(&mut session, &main, true);
        assert!(!has_export(&patched, "helper"));
        let (_, _, incremental_wasm) = session.incremental_stats(&main);
        assert!(
            incremental_wasm,
            "same-option rebuilds should keep incremental emission"
        );
    }

    #[test]
    fn development_dwarf_is_equivalent_across_incremental_and_cold_builds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let initial = "function answer(): i32\n    return 41\nend\n";
        let changed_source = "function answer(): i32\n    return 42\nend\n";
        let main = write(dir.path(), "main.walu", initial);
        let options = crate::CompileOptions {
            development_dwarf: true,
            ..Default::default()
        };
        let mut session = CompilerSession::new();
        assert!(
            session
                .build_root_with_options(&main, "program.wasm", options)
                .artifacts
                .is_some()
        );
        let canonical_main = main.canonicalize().expect("canonicalize");
        session.set_overlay(&canonical_main, changed_source);
        let incremental = session
            .build_root_with_options(&main, "program.wasm", options)
            .artifacts
            .expect("incremental artifacts")
            .wasm;
        assert!(session.incremental_stats(&main).2);

        fs::write(&main, changed_source).expect("changed fixture should write");
        let cold = CompilerSession::new()
            .build_root_with_options(&main, "program.wasm", options)
            .artifacts
            .expect("cold artifacts")
            .wasm;
        assert_eq!(
            incremental, cold,
            "debug metadata must use final cached offsets"
        );

        let production = session
            .build_root_with_options(&main, "program.wasm", crate::CompileOptions::default())
            .artifacts
            .expect("option change should rebuild")
            .wasm;
        assert!(
            !production
                .windows(b".debug_info".len())
                .any(|window| window == b".debug_info")
        );
        assert!(
            !session.incremental_stats(&main).2,
            "debug configuration must participate in cache identity"
        );
    }

    #[test]
    fn signature_edit_falls_back_to_full_lowering_and_emission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = write(
            dir.path(),
            "main.walu",
            "function changed(): i32\n    return 32\nend\n",
        );
        let mut session = CompilerSession::new();
        assert!(
            session
                .build_root(&main, "program.wasm")
                .artifacts
                .is_some()
        );
        let canonical_main = main.canonicalize().expect("canonicalize");
        session.set_overlay(
            &canonical_main,
            "function changed(): f64\n    return 32.0\nend\n",
        );
        let changed = session.build_root(&main, "program.wasm");
        assert!(changed.diagnostics.is_empty(), "{:?}", changed.diagnostics);
        let (_, incremental_ir, incremental_wasm) = session.incremental_stats(&main);
        assert!(
            !incremental_ir,
            "signature changes require full IR lowering"
        );
        assert!(
            !incremental_wasm,
            "signature changes require full Wasm emission"
        );
    }
}
