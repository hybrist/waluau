---
name: pr-watch-merge
description: Use when the user asks the agent to monitor the PR for the current branch, actively react to action failures or merge conflicts, and continue until the PR is merged.
---

# PR Watch Until Merged

Drive a branch PR to merge completion. Do not stop after opening or checking the PR; continue until merged.

## When to use

Use this skill when the user asks to:
- watch or babysit a PR
- react to CI/GitHub Action failures
- handle merge conflicts
- stay on the task until merge is complete

## Core behavior

1. Resolve branch and PR context:
- Determine current branch (`git branch --show-current`).
- Find its PR (prefer `gh pr view --json` on the current branch).
- If no PR exists, report that as a blocker and create one only if explicitly requested.

2. Monitor merge readiness in a loop until merged:
- Poll PR state, mergeability, review status, and check status.
- If checks are failing, inspect failed jobs/logs, fix root cause locally, commit, push, and re-check.
- If PR is behind or conflicted, `git fetch origin main` and merge `origin/main` into the branch (never rebase), resolve conflicts, run tests, commit, push, and re-check.
- If approvals or required reviewers are missing and the agent cannot self-resolve, surface exactly what is missing.
- Keep iterating until PR state is `MERGED`.

Conflict-resolution policy for this repo:
- Resolve PR conflicts by fetching from remote and merging `origin/main` into the PR branch; do not rebase.
- Do not rely on local `main` for conflict resolution. Always fetch and use the latest remote `main`.
- All PRs are merged via squash merge, so PR branch commit history is not important.
- Merge commits on PR branches are acceptable.

3. Favor non-interactive, automatable commands:
- Use JSON/parsable output flags where available.
- Avoid commands that open interactive editors or prompts.
- Re-run focused quality gates after each fix before pushing.

4. Stop condition:
- Only stop once PR is confirmed merged.

## Post-merge requirement (mandatory)

After merge:
1. Update local refs and switch to latest `main` (`git fetch`, checkout `main`, fast-forward/rebase).
2. Review latest main branch state for unfinished or newly exposed follow-up work.
3. Check beads for existing related items (`bd ready`, `bd search`, `bd show` as needed).
4. If follow-up is needed and not tracked, create beads issue(s) with clear description and dependencies (`discovered-from` when applicable).
5. If no follow-up is needed, explicitly state that verification was performed and nothing new was required.

## Reporting format

Provide concise status updates each cycle:
- PR state: open/merged, mergeability, checks summary
- Actions taken: fixes/conflict resolution/retest/push
- Remaining blockers: exact required condition (if any)
- Final: merged confirmation + post-merge beads verification result
