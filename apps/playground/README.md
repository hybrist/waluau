# Waluau Playground

The playground is a Vite app that loads the `waluau-wasm` compiler artifact built from the Rust workspace.

## Local development

From the repository root:

```bash
pnpm install
pnpm dev
```

The dev server uses a Vite plugin that builds the Rust compiler crate and runs `wasm-bindgen` before serving. Rust source changes trigger an automatic rebuild and full reload.

## Compiler worker

The editor, REPL, and language features share a dedicated module worker that
owns the `waluau-wasm` compiler and language-server session. Project snapshots
are analyzed asynchronously off the browser's main thread. The combined
analysis call links and typechecks each reachable module graph once, reuses the
typed entry program for code generation, and checks only uncovered files as
additional roots. Rapid edits are coalesced to the latest pending snapshot.

## Production build

From the repository root:

```bash
pnpm build:playground
```

This runs a release-mode Wasm build through the same Vite plugin, then emits the site into `apps/playground/dist`. No separate `build:wasm` step is required.

## Browser tests

From the repository root:

```bash
pnpm test:playground
```

Playwright builds the playground before starting its preview server so tests
always exercise the checked-out sources instead of a stale, ignored `dist`
directory. Set `PLAYWRIGHT_SKIP_BUILD=1` only when the same workflow has already
completed a successful production build, as the playground CI job does.

## Vercel

The repository includes a root-level [`vercel.json`](../../vercel.json) so Vercel can deploy the playground directly from the monorepo root:

- `installCommand`: `pnpm install --frozen-lockfile`
- `buildCommand`: `pnpm build:playground`
- `outputDirectory`: `apps/playground/dist`

Create a single Vercel project connected to this repository and keep its Root Directory at the repository root so the build can access the Rust workspace.
