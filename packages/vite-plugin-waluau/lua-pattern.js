// Lua pattern matching engine: a direct port of the matcher in Luau's
// lstrlib.c (itself Lua 5.x). Operates on JS strings per UTF-16 code unit,
// which matches the runtime's JS-string encoding; character classes are
// ASCII-only, mirroring the C locale used by Lua.
//
// Like Lua, malformed patterns are only reported when the matcher actually
// reaches the malformed piece, so there is no separate validation pass.
//
// All indices at this module's API boundary are 0-based JS offsets; the
// callers in wasm.js translate to/from Lua's 1-based convention.

const MAXCAPTURES = 32;
const MAXCCALLS = 200;
const L_ESC = '%';

const CAP_UNFINISHED = -1;
const CAP_POSITION = -2;

export class LuaPatternError extends Error {}

function patternError(message) {
  throw new LuaPatternError(message);
}

function isalpha(c) { return (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a); }
function isdigit(c) { return c >= 0x30 && c <= 0x39; }
function isalnum(c) { return isalpha(c) || isdigit(c); }
function isupper(c) { return c >= 0x41 && c <= 0x5a; }
function islower(c) { return c >= 0x61 && c <= 0x7a; }
function iscntrl(c) { return c < 0x20 || c === 0x7f; }
function isspace(c) { return c === 0x20 || (c >= 0x09 && c <= 0x0d); }
function isgraph(c) { return c > 0x20 && c < 0x7f; }
function ispunct(c) { return isgraph(c) && !isalnum(c); }
function isxdigit(c) {
  return isdigit(c) || (c >= 0x41 && c <= 0x46) || (c >= 0x61 && c <= 0x66);
}

// Matches lstrlib's match_class: `clCode` is the class letter's char code;
// an uppercase letter negates, a non-letter matches itself literally.
function classMatch(code, clCode) {
  let res;
  switch (clCode | 0x20) { // tolower
    case 0x61: res = isalpha(code); break; // a
    case 0x63: res = iscntrl(code); break; // c
    case 0x64: res = isdigit(code); break; // d
    case 0x67: res = isgraph(code); break; // g
    case 0x6c: res = islower(code); break; // l
    case 0x70: res = ispunct(code); break; // p
    case 0x73: res = isspace(code); break; // s
    case 0x75: res = isupper(code); break; // u
    case 0x77: res = isalnum(code); break; // w
    case 0x78: res = isxdigit(code); break; // x
    case 0x7a: res = code === 0; break; // z (zero byte, kept from Lua 5.1)
    default: return clCode === code;
  }
  return isupper(clCode) ? !res : res;
}

class MatchState {
  constructor(source, pattern) {
    this.src = source;
    this.p = pattern;
    this.level = 0;
    this.capture = []; // { init: number, len: number }
    this.matchdepth = MAXCCALLS;
  }
}

function checkCapture(ms, l) {
  const index = l - 0x31; // '1'
  if (index < 0 || index >= ms.level || ms.capture[index].len === CAP_UNFINISHED) {
    patternError(`invalid capture index %${index + 1}`);
  }
  return index;
}

function captureToClose(ms) {
  for (let level = ms.level - 1; level >= 0; level--) {
    if (ms.capture[level].len === CAP_UNFINISHED) return level;
  }
  patternError('invalid pattern capture');
}

// Returns the index just past a single-char pattern item starting at p
// (a literal, %x escape, or [set] class).
function classEnd(ms, p) {
  const pat = ms.p;
  const c = pat[p++];
  if (c === L_ESC) {
    if (p >= pat.length) patternError("malformed pattern (ends with '%')");
    return p + 1;
  }
  if (c === '[') {
    if (pat[p] === '^') p++;
    do { // look for a ']'
      if (p >= pat.length) patternError("malformed pattern (missing ']')");
      if (pat[p++] === L_ESC && p < pat.length) p++; // skip escapes like '%]'
    } while (pat[p] !== ']');
    return p + 1;
  }
  return p;
}

// `p` points at the '[' and `ec` at the matching ']'.
function matchBracketClass(code, ms, p, ec) {
  const pat = ms.p;
  let sig = true;
  if (pat[p + 1] === '^') {
    sig = false;
    p++; // skip the '^'
  }
  while (++p < ec) {
    if (pat[p] === L_ESC) {
      p++;
      if (classMatch(code, pat.charCodeAt(p))) return sig;
    } else if (pat[p + 1] === '-' && p + 2 < ec) {
      if (pat.charCodeAt(p) <= code && code <= pat.charCodeAt(p + 2)) return sig;
      p += 2;
    } else if (pat.charCodeAt(p) === code) {
      return sig;
    }
  }
  return !sig;
}

// Whether the source char at s matches the single-char item at pattern
// offset p (whose end is ep).
function singleMatch(ms, s, p, ep) {
  if (s >= ms.src.length) return false;
  const code = ms.src.charCodeAt(s);
  switch (ms.p[p]) {
    case '.': return true;
    case L_ESC: return classMatch(code, ms.p.charCodeAt(p + 1));
    case '[': return matchBracketClass(code, ms, p, ep - 1);
    default: return ms.p.charCodeAt(p) === code;
  }
}

function matchBalance(ms, s, p) {
  if (p + 1 >= ms.p.length) {
    patternError("malformed pattern (missing arguments to '%b')");
  }
  if (s >= ms.src.length || ms.src[s] !== ms.p[p]) return -1;
  const b = ms.p[p];
  const e = ms.p[p + 1];
  let cont = 1;
  let i = s;
  while (++i < ms.src.length) {
    if (ms.src[i] === e) {
      if (--cont === 0) return i + 1;
    } else if (ms.src[i] === b) {
      cont++;
    }
  }
  return -1; // string ends out of balance
}

function maxExpand(ms, s, p, ep) {
  let i = 0; // count maximum expansion of the single item
  while (singleMatch(ms, s + i, p, ep)) i++;
  // Try to match the rest with i repetitions, backtracking down to 0.
  while (i >= 0) {
    const res = doMatch(ms, s + i, ep + 1);
    if (res !== -1) return res;
    i--;
  }
  return -1;
}

function minExpand(ms, s, p, ep) {
  for (;;) {
    const res = doMatch(ms, s, ep + 1);
    if (res !== -1) return res;
    if (singleMatch(ms, s, p, ep)) s++;
    else return -1;
  }
}

function startCapture(ms, s, p, what) {
  if (ms.level >= MAXCAPTURES) patternError('too many captures');
  ms.capture[ms.level] = { init: s, len: what };
  ms.level++;
  const res = doMatch(ms, s, p);
  if (res === -1) ms.level--; // undo on failure
  return res;
}

function endCapture(ms, s, p) {
  const l = captureToClose(ms);
  ms.capture[l].len = s - ms.capture[l].init;
  const res = doMatch(ms, s, p);
  if (res === -1) ms.capture[l].len = CAP_UNFINISHED; // undo on failure
  return res;
}

function matchCapture(ms, s, l) {
  const index = checkCapture(ms, l);
  const len = ms.capture[index].len;
  const captured = ms.src.substr(ms.capture[index].init, len);
  if (ms.src.length - s >= len && ms.src.substr(s, len) === captured) {
    return s + len;
  }
  return -1;
}

// Core matcher. Returns the source end offset of the match, or -1.
function doMatch(ms, s, p) {
  if (ms.matchdepth-- === 0) patternError('pattern too complex');
  try {
    while (p < ms.p.length) {
      let handled = false;
      switch (ms.p[p]) {
        case '(':
          return ms.p[p + 1] === ')'
            ? startCapture(ms, s, p + 2, CAP_POSITION)
            : startCapture(ms, s, p + 1, CAP_UNFINISHED);
        case ')':
          return endCapture(ms, s, p + 1);
        case '$':
          if (p + 1 === ms.p.length) {
            return s === ms.src.length ? s : -1;
          }
          break; // otherwise '$' is a literal
        case L_ESC:
          switch (ms.p[p + 1]) {
            case 'b': {
              const res = matchBalance(ms, s, p + 2);
              if (res === -1) return -1;
              s = res;
              p += 4;
              handled = true;
              break;
            }
            case 'f': {
              p += 2;
              if (ms.p[p] !== '[') {
                patternError("missing '[' after '%f' in pattern");
              }
              const ep = classEnd(ms, p);
              const previous = s === 0 ? 0 : ms.src.charCodeAt(s - 1);
              const current = s < ms.src.length ? ms.src.charCodeAt(s) : 0;
              if (!matchBracketClass(previous, ms, p, ep - 1) &&
                  matchBracketClass(current, ms, p, ep - 1)) {
                p = ep;
                handled = true;
                break;
              }
              return -1;
            }
            case '0': case '1': case '2': case '3': case '4':
            case '5': case '6': case '7': case '8': case '9': {
              const res = matchCapture(ms, s, ms.p.charCodeAt(p + 1));
              if (res === -1) return -1;
              s = res;
              p += 2;
              handled = true;
              break;
            }
            default:
              break; // regular class escape; handled as a single item below
          }
          break;
        default:
          break;
      }
      if (handled) continue;
      // Single-char pattern item, possibly with a quantifier.
      const ep = classEnd(ms, p);
      switch (ms.p[ep]) {
        case '?': {
          if (singleMatch(ms, s, p, ep)) {
            const res = doMatch(ms, s + 1, ep + 1);
            if (res !== -1) return res;
          }
          p = ep + 1;
          continue;
        }
        case '+':
          return singleMatch(ms, s, p, ep) ? maxExpand(ms, s + 1, p, ep) : -1;
        case '*':
          return maxExpand(ms, s, p, ep);
        case '-':
          return minExpand(ms, s, p, ep);
        default:
          if (!singleMatch(ms, s, p, ep)) return -1;
          s++;
          p = ep;
          continue;
      }
    }
    return s;
  } finally {
    ms.matchdepth++;
  }
}

// One resolved capture: { position: true, value: 1-based index } or
// { position: false, value: string }.
function getOneCapture(ms, i, s, e) {
  if (i >= ms.level) {
    if (i === 0) {
      return { position: false, value: ms.src.slice(s, e) };
    }
    patternError('invalid capture index');
  }
  const { init, len } = ms.capture[i];
  if (len === CAP_UNFINISHED) patternError('unfinished capture');
  if (len === CAP_POSITION) {
    return { position: true, value: init + 1 };
  }
  return { position: false, value: ms.src.substr(init, len) };
}

// All captures for a successful match [s, e); when the pattern has no
// explicit captures, the whole match is the single capture (unless
// wholeMatchFallback is false, as for string.find's extra results).
function pushCaptures(ms, s, e, wholeMatchFallback) {
  const nlevels = ms.level === 0 && wholeMatchFallback ? 1 : ms.level;
  const captures = [];
  for (let i = 0; i < nlevels; i++) {
    captures.push(getOneCapture(ms, i, s, e));
  }
  return captures;
}

// string.find / string.match shared driver. init is a 0-based offset that
// must already be clamped to [0, source.length]. Returns null on no match,
// otherwise { start, end, captures } with 0-based [start, end) offsets.
export function luaPatternMatch(source, pattern, init, wholeMatchFallback) {
  const ms = new MatchState(source, pattern);
  let p = 0;
  const anchor = pattern[0] === '^';
  if (anchor) p = 1;
  let s = init;
  do {
    ms.level = 0;
    ms.matchdepth = MAXCCALLS;
    const e = doMatch(ms, s, p);
    if (e !== -1) {
      return { start: s, end: e, captures: pushCaptures(ms, s, e, wholeMatchFallback) };
    }
  } while (s++ < source.length && !anchor);
  return null;
}

// string.gsub replacement-string application: expands %0-%9 and %% in `repl`
// for a match with the given whole text and captures.
function substituteReplacement(repl, whole, captures) {
  let out = '';
  for (let i = 0; i < repl.length; i++) {
    const ch = repl[i];
    if (ch !== L_ESC) {
      out += ch;
      continue;
    }
    i++;
    const next = repl[i];
    if (next === L_ESC) {
      out += L_ESC;
    } else if (next === '0') {
      out += whole;
    } else if (next >= '1' && next <= '9') {
      const index = next.charCodeAt(0) - 0x31;
      if (index >= captures.length) {
        patternError(`invalid capture index %${index + 1}`);
      }
      const capture = captures[index];
      out += capture.position ? String(capture.value) : capture.value;
    } else {
      patternError("invalid use of '%' in replacement string");
    }
  }
  return out;
}

// string.gsub driver as a two-way generator: yields { whole, captures } per
// match and receives the replacement string back through next() (null or
// undefined keeps the original match text). Returns { result, count }.
// maxN < 0 means unlimited. This shape lets the wasm-side lowering drive the
// loop when the replacement is a guest function.
export function* luaGsubGenerator(source, pattern, maxN) {
  const ms = new MatchState(source, pattern);
  let p = 0;
  const anchor = pattern[0] === '^';
  if (anchor) p = 1;
  let s = 0;
  let n = 0;
  let out = '';
  const limit = maxN < 0 ? Infinity : maxN;
  while (n < limit) {
    ms.level = 0;
    ms.matchdepth = MAXCCALLS;
    const e = doMatch(ms, s, p);
    if (e !== -1) {
      n++;
      const whole = source.slice(s, e);
      const replacement = yield { whole, captures: pushCaptures(ms, s, e, true) };
      out += replacement === null || replacement === undefined ? whole : replacement;
    }
    if (e !== -1 && e > s) {
      s = e; // non-empty match: continue after it
    } else if (s < source.length) {
      out += source[s];
      s++;
    } else {
      break;
    }
    if (anchor) break;
  }
  out += source.slice(s);
  return { result: out, count: n };
}

// string.gsub with a synchronous replacer callback.
export function luaGsub(source, pattern, replacer, maxN) {
  const gen = luaGsubGenerator(source, pattern, maxN);
  let step = gen.next();
  while (!step.done) {
    step = gen.next(replacer(step.value.whole, step.value.captures));
  }
  return step.value;
}

// Builds a replacer for gsub's replacement-string form.
export function makeStringReplacer(repl) {
  return (whole, captures) => substituteReplacement(repl, whole, captures);
}

// string.gmatch driver: returns a stateful next() that yields
// { start, end, captures } per call, or null when exhausted. As in Lua,
// '^' is not an anchor here (it matches a literal '^').
export function luaGmatch(source, pattern) {
  let s = 0;
  const ms = new MatchState(source, pattern);
  return function next() {
    while (s <= source.length) {
      ms.level = 0;
      ms.matchdepth = MAXCCALLS;
      const e = doMatch(ms, s, 0);
      if (e !== -1) {
        const start = s;
        s = e > s ? e : s + 1; // empty match: advance one char
        return { start, end: e, captures: pushCaptures(ms, start, e, true) };
      }
      s++;
    }
    return null;
  };
}
