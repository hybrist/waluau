This repository uses the Beads Dolt upstream as the source of truth.

Each checkout keeps its own embedded Dolt database and local export files under `.beads/`, all untracked except for shared bootstrap config.

Bootstrap a fresh clone or worktree with:

```sh
pnpm install                    # installs the @beads/bd binary
pnpm exec bd bootstrap --yes   # clones Dolt database from git remote
pnpm exec bd ready --json
```

After mutations, sync the upstream state with:

```sh
pnpm exec bd dolt push
```
