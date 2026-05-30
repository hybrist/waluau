#!/bin/bash
set -euo pipefail

# Only run in remote (Claude Code on the web) environments
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

echo '{"async": true, "asyncTimeout": 300000}'

cd "${CLAUDE_PROJECT_DIR:-.}"

# Install bd if not present
# Pinned to 1.0.4: 1.0.5+ has migration 47 incompatible with the current schema v46 remote
BD_VERSION="1.0.4"
if ! command -v bd >/dev/null 2>&1; then
  OS=$(uname -s | tr '[:upper:]' '[:lower:]')
  ARCH=$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
  curl -fsSL "https://github.com/gastownhall/beads/releases/download/v${BD_VERSION}/beads_${BD_VERSION}_${OS}_${ARCH}.tar.gz" \
    | tar -xz -C /usr/local/bin/ bd
fi

# Bootstrap beads Dolt database from git remote if not already present
if ! bd ready >/dev/null 2>&1; then
  bd bootstrap --yes
fi
