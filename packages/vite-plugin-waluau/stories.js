// Static reading of a *.stories.walu file. Storybook's index is built in Node
// without ever instantiating Wasm, so the story names have to be readable from
// the source; the generated CSF module is built from the same reading, which
// is what keeps the sidebar and the mountable stories in step.
//
// A story is declared as `storybook.story("<name>", { ... })` with a literal
// name. That is the documented contract, not a heuristic: a name computed at
// runtime cannot appear in a static index.

const STORY_CALL = /(?:^|[^\w.])([A-Za-z_][\w]*)\s*\.\s*story\s*\(\s*(["'])((?:[^\\]|\\.)*?)\2/g;
const LINE_COMMENT = /--(?!\[=*\[)[^\n]*/g;
const BLOCK_COMMENT = /--\[(=*)\[[\s\S]*?\]\1\]/g;

/** Drops comments so a commented-out story never reaches the index. */
function withoutComments(source) {
  return source.replace(BLOCK_COMMENT, '').replace(LINE_COMMENT, '');
}

function unescape(value) {
  return value.replace(/\\(["'\\nrt])/g, (_, escaped) => {
    if (escaped === 'n') return '\n';
    if (escaped === 'r') return '\r';
    if (escaped === 't') return '\t';
    return escaped;
  });
}

/**
 * The local name a story file binds `waluau:engine/storybook` to, so
 * `storybook.story(...)` is recognized under whatever alias the file chose.
 * Returns null when the file does not require the module at all.
 */
export function storybookBinding(source) {
  const require = /local\s+([A-Za-z_][\w]*)\s*=\s*require\s*\(\s*["']waluau:engine(?:\/v1)?\/storybook["']\s*\)/
    .exec(withoutComments(source));
  return require == null ? null : require[1];
}

/**
 * Story names declared by a *.stories.walu source, in declaration order.
 * Duplicates are dropped: two stories with one name cannot be told apart by
 * the host that mounts them by name.
 */
export function parseStoryNames(source) {
  const binding = storybookBinding(source);
  if (binding == null) return [];
  const body = withoutComments(source);
  const names = [];
  const seen = new Set();
  for (const match of body.matchAll(STORY_CALL)) {
    if (match[1] !== binding) continue;
    const name = unescape(match[3]);
    if (name.length === 0 || seen.has(name)) continue;
    seen.add(name);
    names.push(name);
  }
  return names;
}

/**
 * The CSF export name for a story name. Storybook derives a display name from
 * the export when a story does not carry one; ours always does, so this only
 * has to be a stable, unique JavaScript identifier.
 */
export function exportNameFor(name, taken = new Set()) {
  const words = name
    .replace(/[^\w\s-]/g, ' ')
    .split(/[\s\-_]+/)
    .filter(Boolean)
    .map((word) => word[0].toUpperCase() + word.slice(1));
  let base = words.join('');
  if (base.length === 0 || /^\d/.test(base)) base = `Story${base}`;
  let candidate = base;
  let suffix = 2;
  while (taken.has(candidate)) {
    candidate = `${base}${suffix}`;
    suffix += 1;
  }
  taken.add(candidate);
  return candidate;
}

/** Story names paired with the CSF export each one is published under. */
export function parseStories(source) {
  const taken = new Set();
  return parseStoryNames(source).map((name) => ({
    name,
    exportName: exportNameFor(name, taken),
  }));
}
