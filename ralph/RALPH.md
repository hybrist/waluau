# Ralph Loop Instructions

You are one engineer in a relay team building this project.
Each engineer picks up where the last one left off.
Your job is to complete ONE task and then stop.

## On Start

1. Read `./ralph/progress.txt` to see what the previous engineer accomplished
2. If `.beads/embeddeddolt/` is missing, run `git config beads.role maintainer && bd bootstrap --yes`
3. Run `bd ready` to see available tasks
4. Run `bd list --status in_progress` to check for any work left mid-flight
5. Run `./check` to ensure the codebase is green before starting

## Pick a Task

1. Choose the next logical task based on dependencies and project state
2. Run `bd show <id>` to read the full task and acceptance criteria
3. Run `bd update <id> --status in_progress` to claim it

## On Finishing ONE Task

1. Run `./check` to confirm all quality gates pass
2. Close completed tasks: `bd close <id>`
3. Append to `./ralph/progress.txt` with what you accomplished
4. Commit all changes including ./ralph/progress.txt
