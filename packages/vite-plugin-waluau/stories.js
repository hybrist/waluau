// Static reading of a *.stories.walu file. Storybook's index is built in Node
// without ever instantiating Wasm, so the story names have to be readable from
// the source; the generated CSF module is built from the same reading, which
// is what keeps the sidebar and the mountable stories in step.
//
// A story is declared as `storybook.story("<name>", { ... })` with a literal
// name. That is the documented contract, not a heuristic: a name computed at
// runtime cannot appear in a static index. An optional third argument is a
// literal list of `storybook.range(...)` / `storybook.select(...)` controls;
// the same reading becomes the generated CSF args and argTypes.

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

function stringLiteral(value, context) {
  const match = /^(["'])((?:[^\\]|\\.)*)\1$/.exec(value.trim());
  if (match == null) throw new Error(`${context} must be a string literal`);
  return unescape(match[2]);
}

function integerLiteral(value, context) {
  const match = /^(-?\d+)(?:::i32)?$/.exec(value.trim());
  if (match == null) throw new Error(`${context} must be an i32 literal`);
  const integer = Number(match[1]);
  if (integer < -2147483648 || integer > 2147483647) {
    throw new Error(`${context} is outside the i32 range`);
  }
  return integer;
}

/** Splits commas that are not inside a string, call, or table literal. */
function splitTopLevel(source) {
  const parts = [];
  const stack = [];
  let start = 0;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index];
    if (character === '"' || character === "'") {
      const quote = character;
      index += 1;
      while (index < source.length && source[index] !== quote) {
        if (source[index] === '\\') index += 1;
        index += 1;
      }
      continue;
    }
    if (character === '(' || character === '{' || character === '[') {
      stack.push(character);
      continue;
    }
    if (character === ')' || character === '}' || character === ']') {
      stack.pop();
      continue;
    }
    if (character === ',' && stack.length === 0) {
      parts.push(source.slice(start, index).trim());
      start = index + 1;
    }
  }
  const final = source.slice(start).trim();
  if (final.length > 0) parts.push(final);
  return parts;
}

/** The arguments after a story call's already-matched literal name. */
function storyTail(source, start) {
  const stack = [];
  let segmentStart = start;
  const parts = [];
  for (let index = start; index < source.length; index += 1) {
    const character = source[index];
    if (character === '"' || character === "'") {
      const quote = character;
      index += 1;
      while (index < source.length && source[index] !== quote) {
        if (source[index] === '\\') index += 1;
        index += 1;
      }
      continue;
    }
    if (character === '(' || character === '{' || character === '[') {
      stack.push(character);
      continue;
    }
    if (character === ')' && stack.length === 0) {
      const final = source.slice(segmentStart, index).trim();
      if (final.length > 0) parts.push(final);
      return parts;
    }
    if (character === ')' || character === '}' || character === ']') {
      stack.pop();
      continue;
    }
    if (character === ',' && stack.length === 0) {
      const part = source.slice(segmentStart, index).trim();
      if (part.length > 0) parts.push(part);
      segmentStart = index + 1;
    }
  }
  return parts;
}

function displayName(name) {
  return name
    .replace(/[_-]+/g, ' ')
    .replace(/(?:^|\s)\w/g, (character) => character.toUpperCase());
}

function parseLabels(value, context) {
  const source = value.trim();
  if (!source.startsWith('{') || !source.endsWith('}')) {
    throw new Error(`${context} labels must be a literal string list`);
  }
  const labels = splitTopLevel(source.slice(1, -1))
    .map((label) => stringLiteral(label, `${context} label`));
  if (labels.length === 0) throw new Error(`${context} must have at least one label`);
  return labels;
}

function parseControls(value, binding, storyName) {
  if (value == null) return [];
  const source = value.trim();
  if (!source.startsWith('{') || !source.endsWith('}')) {
    throw new Error(`Controls for story "${storyName}" must be a literal list`);
  }
  const controls = [];
  const names = new Set();
  for (const declaration of splitTopLevel(source.slice(1, -1))) {
    const pattern = new RegExp(`^${binding}\\s*\\.\\s*(range|select)\\s*\\(([\\s\\S]*)\\)$`);
    const match = pattern.exec(declaration);
    if (match == null) {
      throw new Error(
        `Controls for story "${storyName}" must use ${binding}.range(...) or ${binding}.select(...)`,
      );
    }
    const kind = match[1];
    const args = splitTopLevel(match[2]);
    const name = stringLiteral(args[0] ?? '', `${kind} control name`);
    if (name.length === 0) throw new Error(`A control for story "${storyName}" has an empty name`);
    if (names.has(name)) throw new Error(`Story "${storyName}" repeats control "${name}"`);
    names.add(name);
    const initial = integerLiteral(args[1] ?? '', `${name} initial value`);

    if (kind === 'range') {
      if (args.length !== 5) {
        throw new Error(`${binding}.range(name, initial, min, max, step) expects five arguments`);
      }
      const min = integerLiteral(args[2], `${name} minimum`);
      const max = integerLiteral(args[3], `${name} maximum`);
      const step = integerLiteral(args[4], `${name} step`);
      if (min > max) throw new Error(`${name} minimum must not exceed its maximum`);
      if (step <= 0) throw new Error(`${name} step must be positive`);
      if (initial < min || initial > max) {
        throw new Error(`${name} initial value must be between its minimum and maximum`);
      }
      controls.push({
        name,
        initial,
        argType: { name: displayName(name), control: { type: 'range', min, max, step } },
      });
      continue;
    }

    if (args.length !== 3) {
      throw new Error(`${binding}.select(name, initial, labels) expects three arguments`);
    }
    const labels = parseLabels(args[2], `${name} select control`);
    if (initial < 0 || initial >= labels.length) {
      throw new Error(`${name} initial value must select one of its labels`);
    }
    const options = labels.map((_, index) => index);
    controls.push({
      name,
      initial,
      argType: {
        name: displayName(name),
        options,
        control: {
          type: 'select',
          labels: Object.fromEntries(labels.map((label, index) => [index, label])),
        },
      },
    });
  }
  return controls;
}

function storyDeclarations(source) {
  const binding = storybookBinding(source);
  if (binding == null) return [];
  const body = withoutComments(source);
  const declarations = [];
  const seen = new Set();
  for (const match of body.matchAll(STORY_CALL)) {
    if (match[1] !== binding) continue;
    const name = unescape(match[3]);
    if (name.length === 0 || seen.has(name)) continue;
    seen.add(name);
    const tail = storyTail(body, match.index + match[0].length);
    declarations.push({ name, controls: parseControls(tail[1], binding, name) });
  }
  return declarations;
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
  return storyDeclarations(source).map(({ name }) => name);
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
  return storyDeclarations(source).map(({ name, controls }) => {
    const story = { name, exportName: exportNameFor(name, taken) };
    if (controls.length === 0) return story;
    story.args = Object.fromEntries(controls.map((control) => [control.name, control.initial]));
    story.argTypes = Object.fromEntries(controls.map((control) => [control.name, control.argType]));
    return story;
  });
}
