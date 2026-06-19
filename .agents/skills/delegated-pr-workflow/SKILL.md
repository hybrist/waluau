---
name: delegated-pr-workflow
description: Use when the user asks to use subagents, parallel agents, branch/PR-based execution, or the repo's established delegated workflow for implementation, investigation, verification, and merge-through-PR work.
---

# Delegated PR Workflow

Use this workflow for multi-step repo work that should be split across subagents
and merged through pull requests.

## Core Rules

- Treat the primary checkout as coordination-only. Use it for `bd`, `git pull`,
  status checks, and handoff verification.
- Do implementation, investigation docs, and test changes in separate worktrees
  under `/private/tmp` on task-specific branches.
- Track all work in beads. Create or claim a bead before editing, use `--json`
  for programmatic commands, close beads only after the relevant PR is merged,
  and push Dolt state.
- Every code or docs change goes through a branch, PR, GitHub checks, and merge.
  Do not push directly to `main`.
- Use subagents for bounded ownership. Give each worker a concrete bead, branch
  name, worktree path, file/module responsibility, validation commands, and final
  report requirements.
- Tell workers they are not alone in the codebase. They must not revert others'
  edits and must rebase or adapt to merged work.

## Starting

1. Run `bd prime`.
2. Refresh coordination checkout: `git status --short --branch` and
   `git pull --rebase`.
3. Inspect the target bead or dependency chain with `bd show <id> --json` and
   `bd ready --json`.
4. If work is missing from beads, create a bead with acceptance criteria before
   editing.
5. Decide the dependency order. Run independent agents in parallel only when
   their write scopes do not overlap.

## Delegating

For each worker prompt, include:

- bead id(s) to claim and close
- branch name and `/private/tmp/...` worktree path
- explicit ownership boundaries
- relevant design docs, prior PRs, and constraints
- validation commands and expected PR/check behavior
- instruction to merge when checks are green and then push beads/Dolt
- final report fields: PR URL, merge commit, bead status, files changed, tests
  run, and follow-up notes

Prefer implementation workers for bounded code changes. Use investigation
workers when design or API exploration is needed; require them to record outcomes
in docs or bead design notes and create follow-up beads for discovered work.

## During Work

- While workers run, do only non-overlapping coordination work.
- Pull latest `main` in the primary checkout after each merge.
- Before starting dependent work, verify the blocking bead is closed and the
  merged code is on `origin/main`.
- If a worker discovers a real follow-up, ensure it is tracked with
  `discovered-from:<parent>` and decide whether it belongs to the current run.

## Completion

1. Verify all requested beads are closed with `bd show` or `bd list`.
2. Push beads: `bd dolt push`.
3. Run final git checks from the primary checkout:
   - `git pull --rebase`
   - `git push`
   - `git status --short --branch`
4. Final response should summarize merged PRs, closed beads, follow-ups created,
   validation, and final checkout state.
