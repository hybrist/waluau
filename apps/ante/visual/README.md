# Ante appearance tests

`pnpm --filter ante test:visual` builds Storybook and compares six isolated
WebGL2 scenes: the packaged card back, court atlas, four suit shaders, a card
draw mid-flight, focused/selected hand layout, and the shop. Gameplay runs
separately with `test:e2e`; it does not use these images or their readiness flag.
These are appearance baselines for this environment, not cross-driver graphics
conformance assertions.

The capture environment is **Playwright 1.60.0 Chromium, Linux amd64, Noble** in
`mcr.microsoft.com/playwright:v1.60.0-noble@sha256:9bd26ad900bb5e0f4dee75839e957a89ae89c2b7ab1e76050e559790e946b948`.
CI uses this image and the lockfile. SwiftShader still executes the production
WebGL2 shaders; viewport is 1200×800, DPR is 1, locale en-US, timezone UTC.
The canvas alone is captured, avoiding Storybook chrome and host system fonts.
The font, atlases, textures and audio are the game's checked-in asset bundle.
The Storybook-only resource barrier fails on asset/shader errors and requires
font/texture upload before capture. A seeded host RNG is fixed at 12345; current
scenes have no randomized game simulation.

The harness owns requestAnimationFrame before navigation. Asset loading uses
wall time while preparation draws remain at timestamp zero. After resources
are ready, it advances explicit 60 Hz steps to the named presentation instant.
The card flight uses the story's scrub control at 45%; shader time is 250 ms.
No screenshot equality or elapsed sleep determines scene readiness.

## Baseline review and updates

Use the exact image above even on macOS. Start a remote browser in one terminal:

```sh
docker run --rm --platform linux/amd64 -p 3000:3000 \
  mcr.microsoft.com/playwright:v1.60.0-noble@sha256:9bd26ad900bb5e0f4dee75839e957a89ae89c2b7ab1e76050e559790e946b948 \
  sh -c 'npx -y playwright@1.60.0 run-server --port 3000 --host 0.0.0.0'
```

Then run from the repository root (Docker Desktop/OrbStack host networking):

```sh
ANTE_VISUAL_WS=ws://127.0.0.1:3000/ ANTE_VISUAL_HOST=host.docker.internal \
  pnpm --filter ante test:visual --update-snapshots
```

Omit `--update-snapshots` for comparison. `PLAYWRIGHT_SKIP_BUILD=1` reuses an
already built Storybook. On the pinned Linux CI container no remote variables
are needed. Local native browsers are useful for diagnosis but must not produce
review baselines. Change the image and Playwright lockfile together when upgrading.

Inspect every changed PNG in `visual/baselines/`, confirm each intentional
appearance change, and include those PNGs in the implementation PR. Run the
comparison twice without updating to confirm reproducibility. Zero changed
pixels are accepted: the renderer/environment are pinned. Never increase a
threshold merely to make an unexplained diff pass.

Failures retain Playwright traces and expected/actual/diff PNGs in
`visual-results/` and a browsable HTML report in `visual-report/`; CI uploads both
as `ante-visual-diffs`. Open with `pnpm exec playwright show-report visual-report`
from `apps/ante`. A mutation check can temporarily change a story background or
card color, rebuild, and confirm comparison fails with an actual/diff image;
restore the source and rebuild before committing. Baseline generation is never
part of CI.
