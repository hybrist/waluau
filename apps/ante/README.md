# Ante

A minimal Vite app whose application code is entirely Waluau. The
`@waluau/vite-plugin` entry in `vite.config.js` compiles `src/main.walu`,
injects the browser runtime, and makes the generated canvas fill the viewport.

```sh
pnpm dev:ante
```

The example creates one custom GPU shader and draws a single full-screen
rectangle through it.
