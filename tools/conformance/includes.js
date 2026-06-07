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
