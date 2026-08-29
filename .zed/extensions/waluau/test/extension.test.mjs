import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const extensionRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
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
  assert.doesNotMatch(highlights, /source\.lua/);
});

test("uses the repository LSP launcher by default", () => {
  const client = read("src/lib.rs");
  const settings = read(path.join("..", "..", "settings.json"));

  assert.match(client, /tools\/editors\/waluau-lsp/);
  assert.match(client, /LspSettings::for_worktree\("waluau-lsp"/);
  assert.match(settings, /"path"\s*:\s*"tools\/editors\/waluau-lsp"/);
});
