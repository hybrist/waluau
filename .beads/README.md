This repository tracks the shared Beads project queue via `.beads/issues.jsonl`.

Each checkout keeps its own embedded Dolt database under `.beads/embeddeddolt/`, which stays untracked via `.beads/.gitignore`.

Bootstrap a fresh clone or worktree with:

```sh
git config beads.role maintainer
bd bootstrap --yes
bd ready --json
```

After mutations, sync both stores:

```sh
bd dolt push
git add .beads/issues.jsonl
git commit -m "Update beads export"
```
