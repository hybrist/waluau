# Waluau Playground

The playground is a Vite app that loads the `waluau-wasm` compiler artifact built from the Rust workspace.

## Local development

From the repository root:

```bash
pnpm install
pnpm dev
```

The dev server watches Rust sources and rebuilds `waluau_wasm.wasm` automatically.

## Production build

From the repository root:

```bash
pnpm build:playground
```

This builds the Rust wasm target first and then emits the Vite site into `apps/playground/dist`.

## Vercel

The repository includes a root-level [`vercel.json`](../../vercel.json) so Vercel can deploy the playground directly from the monorepo root:

- `installCommand`: `pnpm install --frozen-lockfile`
- `buildCommand`: `pnpm build:playground`
- `outputDirectory`: `apps/playground/dist`

Create a single Vercel project connected to this repository and keep its Root Directory at the repository root so the build can access the Rust workspace.
