export const CONFORMANCE_INCLUDE_DIRECTIVE = /^--\s*conformance:\s*include=(.+)$/gm;

function cleanPath(path) {
  const parts = [];
  for (const part of path.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') parts.pop();
    else parts.push(part);
  }
  return `/${parts.join('/')}`;
}

export function resolveConformanceIncludePath(testName, includePath) {
  const baseDir = testName.includes('/') ? testName.slice(0, testName.lastIndexOf('/')) : '';
  return cleanPath(`/conformance/${baseDir}/${includePath}`);
}

export function conformanceIncludePaths(testName, source) {
  const paths = [];
  for (const match of source.matchAll(CONFORMANCE_INCLUDE_DIRECTIVE)) {
    paths.push(resolveConformanceIncludePath(testName, match[1].trim()));
  }
  return paths;
}

// Marks a test that should pass eventually but does not yet. The runner only
// checks that such a test currently fails (it does not care how).
export const CONFORMANCE_PENDING_DIRECTIVE = /^--\s*conformance:\s*pending\s*$/m;

// One expected fragment of a fail test's failure message. May appear multiple
// times; every fragment must be present in the actual failure.
export const CONFORMANCE_ERROR_DIRECTIVE = /^--\s*conformance:\s*error=(.+)$/gm;

// Parses the pending/fail expectations encoded in a conformance file's source.
// `pending` is true when the file carries a `-- conformance: pending` directive.
// `expectedErrors` lists the `-- conformance: error=` fragments; a non-empty
// list marks the file as a fail test.
export function conformanceExpectations(source) {
  const pending = CONFORMANCE_PENDING_DIRECTIVE.test(source);
  const expectedErrors = [];
  for (const match of source.matchAll(CONFORMANCE_ERROR_DIRECTIVE)) {
    expectedErrors.push(match[1].trim());
  }
  return { pending, expectedErrors };
}

// Collapses runs of whitespace to single spaces and trims, so expected
// fragments can be matched without caring about exact spacing or line breaks.
export function normalizeWhitespace(text) {
  return String(text).replace(/\s+/g, ' ').trim();
}

// Fuzzy match: every expected fragment must appear (whitespace-normalized) as a
// substring of the actual failure message.
export function failureMatchesExpected(message, expectedErrors) {
  const normalized = normalizeWhitespace(message);
  return expectedErrors.every((fragment) => normalized.includes(normalizeWhitespace(fragment)));
}
