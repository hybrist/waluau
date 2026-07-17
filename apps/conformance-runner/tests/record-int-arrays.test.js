import { describe, expect, it } from 'vitest';
import { compileAndInstantiateWithExports } from '../src/runner.js';

// Mirrors the engine's font flow: font_from_resource builds an {i32} array
// from host calls, hands it over inside a record, set_font_resource copies it
// into a mutable state record, and a state method looks values up 0-based.
const probeSource = `
type Font = {
    first_code: i32,
    advance: i32,
    advances: {i32}
}

type State = {
    first_code: i32,
    advance: i32,
    advances: {i32}
}

declare function probe_value(index: i32): i32

function load_font(count: i32): Font
    local advances: {i32} = {}
    local last: i32 = count - 1
    for index = 0::i32, last do
        local value: i32 = probe_value(index)
        table.insert(advances, value)
    end
    return { first_code = 32, advance = 64, advances = advances }
end

function State:apply(font: Font): unit
    self.first_code = font.first_code
    self.advance = font.advance
    self.advances = font.advances
end

function State:glyph_advance(code: i32): i32
    local index: i32 = code - self.first_code
    if index >= 0 and index < #self.advances then
        return self.advances[index]
    end
    return self.advance
end

function run(): i32
    local state: State = { first_code = 0, advance = 0, advances = {} }
    local font: Font = load_font(5)
    state:apply(font)
    -- advances = {100, 101, 102, 103, 104}, first_code 32
    local length: i32 = #state.advances
    local first: i32 = state:glyph_advance(32)
    local last: i32 = state:glyph_advance(36)
    local fallback: i32 = state:glyph_advance(37)
    return length * 1000000 + first * 1000 + last + fallback
end
`;

describe('{i32} arrays carried through records', () => {
  it('builds from host calls, copies via a method, and reads 0-based with fallback', async () => {
    const exports = await compileAndInstantiateWithExports({ '/main.walu': probeSource }, '/main.walu', {
      hostImports: {
        probe_value: (index) => 100 + Number(index),
      },
    });
    // length=5 -> 5000000, first=100 -> 100000, last=104, fallback=64
    expect(exports.run()).toBe(5000000 + 100 * 1000 + 104 + 64);
  });
});
