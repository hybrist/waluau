import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const extensionRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(extensionRoot, "..", "..", "..");
const grammarRoot = path.join(repoRoot, "tools", "tree-sitter-waluau");
const read = (relativePath) =>
  fs.readFileSync(path.join(extensionRoot, relativePath), "utf8");

test("registers a first-class Waluau language and language server", () => {
  const manifest = read("extension.toml");
  const language = read("languages/waluau/config.toml");
  const settings = read(path.join("..", "..", "settings.json"));

  assert.match(manifest, /^id = "waluau"$/m);
  assert.match(manifest, /^languages = \["Waluau"\]$/m);
  assert.match(manifest, /^Waluau = "waluau"$/m);
  assert.match(language, /^name = "Waluau"$/m);
  assert.match(language, /^path_suffixes = \["walu"\]$/m);
  assert.match(settings, /"Waluau"\s*:\s*\["walu"\]/);
  assert.match(settings, /"Waluau"\s*:\s*\{[\s\S]*?"waluau-lsp"/);
  assert.doesNotMatch(settings, /"Lua"/);
});

test("uses the repository's own Waluau grammar", () => {
  const manifest = read("extension.toml");
  const language = read("languages/waluau/config.toml");

  assert.match(manifest, /^\[grammars\.waluau\]$/m);
  assert.match(manifest, /^repository = "https:\/\/github\.com\/hybrist\/waluau"$/m);
  assert.match(manifest, /^path = "tools\/tree-sitter-waluau"$/m);
  assert.match(manifest, /^commit = "[0-9a-f]{40}"$/m);
  assert.match(language, /^grammar = "waluau"$/m);

  // No trace of the previously pinned Luau grammar remains.
  assert.doesNotMatch(manifest, /tree-sitter-luau|4teapo/);
  assert.ok(!fs.existsSync(path.join(extensionRoot, "THIRD_PARTY_NOTICES.md")));

  // The in-repo grammar the manifest points at is complete: Zed compiles
  // from the committed src/ directory.
  for (const file of [
    "grammar.js",
    "src/parser.c",
    "src/scanner.c",
    "src/grammar.json",
    "src/node-types.json",
  ]) {
    assert.ok(fs.existsSync(path.join(grammarRoot, file)), `missing ${file}`);
  }
});

test("queries only reference nodes the Waluau grammar defines", () => {
  const nodeTypes = JSON.parse(
    fs.readFileSync(path.join(grammarRoot, "src", "node-types.json"), "utf8"),
  );
  const known = new Set();
  const collect = (entry) => {
    known.add(entry.type);
    for (const value of Object.values(entry.fields ?? {})) {
      for (const child of value.types ?? []) known.add(child.type);
    }
    for (const child of entry.children?.types ?? []) known.add(child.type);
    for (const subtype of entry.subtypes ?? []) known.add(subtype.type);
  };
  nodeTypes.forEach(collect);

  const queryDir = path.join(extensionRoot, "languages", "waluau");
  for (const file of fs.readdirSync(queryDir).filter((f) => f.endsWith(".scm"))) {
    const source = fs.readFileSync(path.join(queryDir, file), "utf8");
    const withoutComments = source.replace(/;[^\n]*/g, "");
    for (const match of withoutComments.matchAll(/\(([a-z_]+)[\s)]/g)) {
      const node = match[1];
      if (node === "_") continue; // wildcard pattern
      assert.ok(known.has(node), `${file} references unknown node (${node})`);
    }
  }
});

test("highlights Waluau-specific syntax", () => {
  const highlights = read("languages/waluau/highlights.scm");

  for (const keyword of [
    "case",
    "const",
    "declare",
    "enum",
    "export",
    "extends",
    "match",
    "opaque",
    "property",
    "type",
  ]) {
    assert.match(highlights, new RegExp(`\\"${keyword}\\"`));
  }

  for (const primitive of [
    "bool",
    "bytes",
    "extern",
    "f32",
    "f64",
    "i32",
    "i64",
    "number",
    "string",
    "thread",
    "u32",
    "u64",
    "unit",
    "unknown",
    "void",
  ]) {
    assert.match(highlights, new RegExp(`\\"${primitive}\\"`));
  }

  assert.match(highlights, /"is"/);
  assert.match(highlights, /tagged_variant_type/);
  assert.match(highlights, /cast_binding/);
  assert.match(highlights, /interpolation/);
  assert.doesNotMatch(highlights, /source\.lua/);
});

test("uses the repository LSP launcher by default", () => {
  const client = read("src/lib.rs");
  const settings = read(path.join("..", "..", "settings.json"));

  assert.match(client, /tools\/editors\/waluau-lsp/);
  assert.match(client, /LspSettings::for_worktree\("waluau-lsp"/);
  assert.match(settings, /"path"\s*:\s*"tools\/editors\/waluau-lsp"/);
});
