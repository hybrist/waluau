# Ante

A minimal Vite app whose application code is entirely Waluau. The
`index.html` file points its normal module script directly at `src/main.walu`.
`@waluau/vite-plugin` compiles that import, supplies the browser runtime, and
makes the generated canvas fill the viewport.

```sh
pnpm dev:ante
```

The example creates one custom GPU shader and draws a single full-screen
rectangle through it.

Tests are Waluau too: `src/gradient.test.walu` covers the gradient math in
`src/gradient.walu` using the `waluau:vitest` API, and vitest runs it in a
headless browser:

```sh
pnpm test:ante
```
