#!/bin/bash
set -euo pipefail

# Only run in remote (Claude Code on the web) environments
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

echo '{"async": true, "asyncTimeout": 300000}'

cd "${CLAUDE_PROJECT_DIR:-.}"

# 1. Install npm deps (gets @beads/bd package + triggers postinstall binary download)
pnpm install --silent 2>/dev/null || pnpm install

# 2. If postinstall didn't place the binary, download it manually via curl
if ! ./node_modules/.bin/bd --version >/dev/null 2>&1; then
  V=$(node -e "console.log(require('./node_modules/@beads/bd/package.json').version)")
  OS=$(uname -s | tr '[:upper:]' '[:lower:]')
  ARCH=$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
  BIN_DIR=$(node -e "console.log(require.resolve('@beads/bd').replace('bin/bd.js','bin/'))" 2>/dev/null || echo "./node_modules/@beads/bd/bin/")
  curl -fsSL "https://github.com/gastownhall/beads/releases/download/v${V}/beads_${V}_${OS}_${ARCH}.tar.gz" \
    | tar -xz -C "$BIN_DIR" bd
  chmod +x "${BIN_DIR}bd"
fi

# 3. Add node_modules/.bin to PATH for the rest of this session so `bd` works unqualified
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  echo "export PATH=\"${CLAUDE_PROJECT_DIR}/node_modules/.bin:\$PATH\"" >> "$CLAUDE_ENV_FILE"
fi

# 4. Bootstrap beads Dolt database from git remote if not already present
if ! ./node_modules/.bin/bd ready >/dev/null 2>&1; then
  ./node_modules/.bin/bd bootstrap --yes
fi
