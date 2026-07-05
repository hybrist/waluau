import { conformanceIncludePaths } from '../../../../tools/conformance/includes.js';
import kanbanAppSource from '../../../../fixtures/kanban/app.walu?raw';

const fixtureModules = import.meta.glob('../../../../fixtures/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const moduleFixtures = import.meta.glob('../../../../fixtures/modules/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const snakeFixtures = import.meta.glob('../../../../fixtures/snake/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const conformanceModules = import.meta.glob('../../../../conformance/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const conformanceIncludeModules = import.meta.glob('../../../../{builtins,externs}/**/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

export { fixtureModules, moduleFixtures, snakeFixtures, conformanceModules };

export function filesForConformancePreset(filename, source) {
  const files = {
    [`/${filename}`]: source
  };

  for (const resolved of conformanceIncludePaths(filename, source)) {
    const includeSource = conformanceIncludeModules[`../../../..${resolved}`];
    if (includeSource === undefined) {
      throw new Error(`Unknown conformance include resolved to ${resolved}`);
    }
    files[resolved] = includeSource;
  }

  return files;
}

const SINGLE_PRESETS = Object.entries(fixtureModules)
  .map(([path, source]) => {
    const filename = path.split('/').pop();
    const key = filename.replace(/\.walu$/, '');
    const label = key
      .split('-')
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' ');
    
    return {
      key,
      label,
      files: {
        [`/${filename}`]: source
      },
      entryFile: `/${filename}`
    };
  });

const CONFORMANCE_PRESETS = Object.entries(conformanceModules)
  .map(([path, source]) => {
    const filename = path.split('/').pop();
    const key = filename.replace(/\.walu$/, '');
    const label = key
      .split('_')
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' ');
    
    return {
      key: `conformance-${key}`,
      label: `${label} (Test)`,
      files: filesForConformancePreset(filename, source),
      entryFile: `/${filename}`
    };
  });

export const MULTI_PRESET = {
  key: 'require-flow',
  label: 'Require Flow Example',
  files: Object.entries(moduleFixtures).reduce((acc, [path, source]) => {
    const filename = path.split('/').pop();
    acc[`/${filename}`] = source;
    return acc;
  }, {}),
  entryFile: '/main.walu'
};

export const DOM_PRESET = {
  key: 'dom-externs',
  label: 'DOM Externs Example',
  files: {
    '/main.walu': `local window = require("dom:window")
local document = window.document
local storage: Storage = window.local_storage
local output_body: HTMLElement = document.body

local heading: Element = document:create_element("h2")
heading.id = "playground-title"
heading.class_name = "waluau-dom-title generated"
heading:set_attribute("data-source", "waluau")

if HTMLHeadingElement(title) = heading then
    title.inner_text = "Hello from generated DOM externs"
else
    heading.text_content = "Generated DOM extern cast failed"
end

output_body:append_child(heading)

local body: Element = document:create_element("p")
local found: Element? = document:get_element_by_id("playground-title")
storage:remove_item("waluau-playground-dom-preset")
local missing: string? = storage:get_item("waluau-playground-dom-preset")
assert(missing == nil)
storage:set_item("waluau-playground-dom-preset", "persisted")
local saved: string? = storage:get_item("waluau-playground-dom-preset")

if found ~= nil then
    if saved ~= nil then
        body.text_content = found.text_content .. " in a sandboxed output document with " .. saved .. " state"
    else
        body.text_content = "DOM storage failed"
    end
else
    body.text_content = "DOM lookup failed"
end

output_body:append_child(body)

local input_element: Element = document:create_element("input")
if HTMLInputElement(input) = input_element then
    input.value = "typed value"
    local value: Element = document:create_element("span")
    value.id = "input-value"
    value.text_content = input.value
    output_body:append_child(value)
end
`
  },
  entryFile: '/main.walu'
};

export const KANBAN_PRESET = {
  key: 'kanban-board',
  label: 'Kanban Board',
  files: {
    '/app.walu': kanbanAppSource
  },
  entryFile: '/app.walu'
};

export const SNAKE_PRESET = {
  key: 'snake-game',
  label: 'Snake Game',
  files: Object.entries(snakeFixtures).reduce((acc, [path, source]) => {
    const filename = path.split('/').pop();
    acc[`/${filename}`] = source;
    return acc;
  }, {}),
  entryFile: '/main.walu'
};

export const PRESETS = [...SINGLE_PRESETS, MULTI_PRESET, DOM_PRESET, KANBAN_PRESET, SNAKE_PRESET, ...CONFORMANCE_PRESETS].sort((left, right) =>
  left.label.localeCompare(right.label)
);

export const DEFAULT_PRESET = PRESETS[0] || {
  key: 'default',
  label: 'Default',
  files: { '/main.walu': '' },
  entryFile: '/main.walu'
};
