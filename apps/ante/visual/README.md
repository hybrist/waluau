# Ante appearance tests

`pnpm --filter ante test:visual` builds Storybook and compares isolated
WebGL2 entities, complete screens, and explicit effect moments. Gameplay runs
separately with `test:e2e`; it does not use these images or their readiness flag.
These are appearance baselines for this environment, not cross-driver graphics
conformance assertions.

## Coverage and migrated assertions

The shared `src/screens.stories.walu` fixture draws production menu, city,
duel, and diagnostic renderers with a seeded run. It selects a screen before
capture, bypassing interactive setup in this visual-only fixture. The same
packaged resources and Storybook resource barrier serve every scene. Wide and
tall scenes use the production `layout.update` calculation. Loading and fatal
scenes call `presentation_resources.draw_status`; the loading flag/error belong
to a separate diagnostic instance, so loading the story's real assets still
completes normally. Loading uses the built-in font, as the live gate does before
the display font arrives. No production failure or layout drawing is copied here.

| Former gameplay measurement | Appearance replacement | Semantic coverage that remains |
| --- | --- | --- |
| Gold menu-title ink | `main-menu.png`: title/options on production city map | Main menu heading and named menu actions |
| Cyan/gold vendor stop ink, then its absence after starting | `starting-vendor.png`: selected starting vendor and anchored city stop; `wide-duel.png`: board without map | Starting vendor → Duel screen transition |
| Card-back ink proves asset decode/board arrival | Existing `card-back.png`, plus `wide-duel.png` deck | Packaged PNG network request and Board ready |
| Loading gold line/text and absence of menu title | `loading.png`: production loading presentation | Held font request blocks menu input; fallback permits menu |
| Fatal audio red panel | `fatal-audio.png`: production fatal diagnostic | Audio could not load heading and fatal message |
| Gold modal heading | `help.png`: complete production help modal | Real touch/keyboard/mouse help open/close |
| Red aiming prompt and changed framebuffer when armed | `targeting.png`: Firebolt prompt, aim target, and socket | Targeting/cancellation feedback and unchanged gold |
| Spell framebuffer changes during burning | `firebolt-burning.png`: production burn at 1600 ms | Exact gold/target/deck effects and completion feedback |
| Wide/tall title, card band and pointer regions | `main-menu.png`, `wide-duel.png`, `tall-duel.png` | Real canvas targets at both viewport shapes |
| Matching frames before/after input; returning to baseline on cancellation | Redundant as a rendering claim: equality only guessed readiness. Representative idle/target/burn captures above cover appearance directly. | Board ready, selection, cancellation and resource assertions |

The earlier six entity captures remain: packaged card back, court atlas, four
suit shaders, card draw at 45%, focused/selected hand layout, and the shop.
High-DPI backing-buffer sizes remain host contract assertions in browser tests.
Imported shader compilation/HMR/program lifetime tests remain app renderer
integration; raw WebGL2 graphics conformance belongs to the conformance runner.

The capture environment is **Playwright 1.60.0 Chromium, Linux amd64, Noble** in
`mcr.microsoft.com/playwright:v1.60.0-noble@sha256:9bd26ad900bb5e0f4dee75839e957a89ae89c2b7ab1e76050e559790e946b948`.
CI uses this image and the lockfile. SwiftShader still executes the production
WebGL2 shaders; viewport is 1200×800, DPR is 1, locale en-US, timezone UTC.
The canvas alone is captured, avoiding Storybook chrome and host system fonts.
The font, atlases, textures and audio are the game's checked-in asset bundle.
The Storybook-only resource barrier fails on asset/shader errors and requires
font/texture upload before capture. A seeded host RNG is fixed at 12345; the
whole-screen fixtures also reset Waluau math.randomseed to 12345 and
construct the city with seed 41.

The harness owns requestAnimationFrame before navigation. Asset loading uses
wall time while preparation draws remain at timestamp zero. After resources
are ready, it advances explicit 60 Hz steps to the named presentation instant.
The card flight uses the story's scrub control at 45%; shader time is 250 ms.
The whole-board Firebolt advances 96 explicit simulation frames to 1600 ms;
other screen fixtures capture at zero after selecting their authored state.
No screenshot equality or elapsed sleep determines scene readiness. Capture
uses a page screenshot clipped to the canvas bounds and compares that buffer;
it never asks a locator screenshot to wait for rAF-driven element stability
while the fixture clock is paused.

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
comparison twice without updating to confirm reproducibility. Zero pixels may
exceed the 0.0053 perceptual color threshold. The original entity scenes measured
1/255 channel rounding between native CI amd64 and amd64 emulation on Apple
Silicon. Full-screen text adds at most 2/255 channel differences: the CI capture
of main menu, starting vendor, help and fatal audio had 429, 350, 583 and 342
raw differing pixels respectively, all at text edges. Only 1, 7, 1 and 4 pixels
exceeded the earlier 0.004 threshold after Playwright's antialias handling.
The largest measured normalized YIQ distance was 0.005241463806366468 at fatal
panel text; 0.0053 rounds that measured bound upward. The pixel allowance stays
zero. This is Playwright's perceptual color distance, not a raw channel
tolerance; geometry and shader captures otherwise matched.
Never increase a threshold merely to make an unexplained diff pass.

Failures retain Playwright traces and expected/actual/diff PNGs in
`visual-results/` and a browsable HTML report in `visual-report/`; CI uploads both
as `ante-visual-diffs`. Open with `pnpm exec playwright show-report visual-report`
from `apps/ante`. A mutation check can temporarily change a story background or
card color, rebuild, and confirm comparison fails with an actual/diff image;
restore the source and rebuild before committing. Baseline generation is never
part of CI.
