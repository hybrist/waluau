# walu-test (browser-compile variant)

Waluau testing on vitest lives in `@waluau/vite-plugin` — see
[`packages/vite-plugin-waluau/README.md`](../../packages/vite-plugin-waluau/README.md)
for the API and standard setup, and `apps/ante` for a wired-up example.

This directory holds only `vite-plugin.js`, a variant of the `*.test.walu`
plugin that compiles test files **in the browser** with the app's built
waluau-wasm compiler (instead of ahead of time with the native CLI). The
conformance runner uses it so test compilation exercises the same in-browser
pipeline the playground ships. The vitest bridge itself is shared:
`@waluau/vite-plugin/testing`.
