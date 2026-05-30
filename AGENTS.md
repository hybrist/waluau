# Agent Instructions

This project uses **bd** (beads) for issue tracking. Run `pnpm exec bd prime` for full workflow context.

## Installing bd

`bd` is installed as an npm devDependency (`@beads/bd`). On a fresh clone:

```bash
pnpm install          # installs bd and downloads the binary via postinstall
```

If the binary download fails (e.g. network restrictions), download it manually:

```bash
# Auto-selects the right platform/arch binary
V=$(node -e "console.log(require('./node_modules/@beads/bd/package.json').version)")
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
curl -fsSL "https://github.com/gastownhall/beads/releases/download/v${V}/beads_${V}_${OS}_${ARCH}.tar.gz" \
  | tar -xz -C "$(pnpm exec node -e "console.log(require.resolve('@beads/bd/package.json').replace('/package.json','/bin/'))")" bd
```

Then bootstrap the Dolt database from the git remote on first use:

```bash
pnpm exec bd bootstrap --yes
```

## Quick Reference

```bash
pnpm exec bd ready              # Find available work
pnpm exec bd show <id>          # View issue details
pnpm exec bd update <id> --claim  # Claim work atomically
pnpm exec bd close <id>         # Complete work
pnpm exec bd dolt push          # Push beads data to remote
```

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

<!-- BEGIN BEADS INTEGRATION v:1 profile:full hash:f65d5d33 -->
## Issue Tracking with bd (beads)

**IMPORTANT**: This project uses **bd (beads)** for ALL issue tracking. Do NOT use markdown TODOs, task lists, or other tracking methods.

### Why bd?

- Dependency-aware: Track blockers and relationships between issues
- Git-friendly: Dolt-powered version control with native sync
- Agent-optimized: JSON output, ready work detection, discovered-from links
- Prevents duplicate tracking systems and confusion

### Quick Start

**Bootstrap a fresh clone or worktree once:**

```bash
pnpm install                    # installs bd binary via postinstall
pnpm exec bd bootstrap --yes   # clones Dolt database from git remote
```

**Check for ready work:**

```bash
pnpm exec bd ready --json
```

**Create new issues:**

```bash
pnpm exec bd create "Issue title" --description="Detailed context" -t bug|feature|task -p 0-4 --json
pnpm exec bd create "Issue title" --description="What this issue is about" -p 1 --deps discovered-from:bd-123 --json
```

**Claim and update:**

```bash
pnpm exec bd update <id> --claim --json
pnpm exec bd update bd-42 --priority 1 --json
```

**Complete work:**

```bash
pnpm exec bd close bd-42 --reason "Completed" --json
```

### Issue Types

- `bug` - Something broken
- `feature` - New functionality
- `task` - Work item (tests, docs, refactoring)
- `epic` - Large feature with subtasks
- `chore` - Maintenance (dependencies, tooling)

### Priorities

- `0` - Critical (security, data loss, broken builds)
- `1` - High (major features, important bugs)
- `2` - Medium (default, nice-to-have)
- `3` - Low (polish, optimization)
- `4` - Backlog (future ideas)

### Workflow for AI Agents

1. **Check ready work**: `pnpm exec bd ready` shows unblocked issues
2. **Claim your task atomically**: `pnpm exec bd update <id> --claim`
3. **Work on it**: Implement, test, document
4. **Discover new work?** Create linked issue:
   - `pnpm exec bd create "Found bug" --description="Details about what was found" -p 1 --deps discovered-from:<parent-id>`
5. **Complete**: `pnpm exec bd close <id> --reason "Done"`

### Quality
- Use `--acceptance` and `--design` fields when creating issues
- Use `--validate` to check description completeness

### Lifecycle
- `pnpm exec bd defer <id>` / `pnpm exec bd supersede <id>` for issue management
- `pnpm exec bd stale` / `pnpm exec bd orphans` / `pnpm exec bd lint` for hygiene
- `pnpm exec bd human <id>` to flag for human decisions
- `pnpm exec bd formula list` / `pnpm exec bd mol pour <name>` for structured workflows

### Auto-Sync

bd automatically syncs via Dolt:

- Each write auto-commits to Dolt history
- The Dolt upstream is the source of truth for the shared queue
- Each checkout keeps its own untracked `.beads/embeddeddolt/` database and local exports
- Use `pnpm exec bd dolt push`/`pnpm exec bd dolt pull` for shared-state sync
- No manual export/import needed!

### Important Rules

- ✅ Use bd for ALL task tracking
- ✅ Always use `--json` flag for programmatic use
- ✅ Link discovered work with `discovered-from` dependencies
- ✅ Check `pnpm exec bd ready` before asking "what should I work on?"
- ❌ Do NOT create markdown TODO lists
- ❌ Do NOT use external issue trackers
- ❌ Do NOT duplicate tracking systems

For more details, see README.md and docs/QUICKSTART.md.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   pnpm exec bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds

<!-- END BEADS INTEGRATION -->
