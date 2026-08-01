# Arcane Heist fixture

A five-breach, player-versus-Arch-Mage trick-taking game built on the Waluau 2D
game engine. It preserves Poker Tricks' deck, exchange, wager, hand-ranking,
computer search, and scoring rules while replacing the casino-table presentation
with a magical robbery inside an arcane vault.

The four suits are now the Red, Blue, Black, and Green schools of magic. Cards
are relics, the shared board is the vault's wards, points are sparks, exchanges
are feints, and each trick is a breach. Poker categories are presented as magical
formations such as a bound pair, arcane sequence, and perfect convergence.
The Arch Mage can commit any valid feint without a spark balance. Its third
breach win ends the heist, while player wins never end the five-breach vault
early, leaving room to maximize the mana carried out of it.

A vault is one heist inside a longer roguelike run. Clearing all five breaches
opens the next vault, which is dealt with whatever mana survived the last one:
the pool is the run's, not the vault's, and every cast spends from it for good.
Between two vaults the run stops at the fence, where that same mana buys spell
upgrades before the next vault is dealt.
The Arch Mage holding three wards ends the whole run, carried mana included, and
the next heist is the first vault of a fresh run rather than a retry. R restarts
the run from vault one at any point, which is also why a settled vault is never
left standing on the board.

The robbers set out with the one spell the menu picked and can hold two ready at
once, on keys 1 and 2. A visit to the fence stocks exactly two offers, drawn
without replacement from everything the run could be sold: one mana off a spell
it carries — four mana for the first step, then six, then eight, and never below
two per cast — or a spell it does not, priced at seven. Buying an offer marks
that line sold and nothing takes its place; what restocks the fence is reaching
the next one. A learned spell takes the next free key at full price; once both
keys are taken, learning a third asks which spell it trades away, and the one
traded out is gone. An offer names the spell it is about rather than the key, so
trading a spell away closes an offer to sharpen it instead of redirecting it.
Losing a vault ends the run and everything it bought with it: a fresh run sets
out with its starting spell at full price again.

Every third vault of a run is a boss battle: both sides are dealt eleven relics
up front and spent pairs are never replaced from the draw pile, so the hands
shrink by two every breach and the fifth breach is fought from a three-card
fan. Everything else — feints, wagers, scoring, and the five breaches — matches
a standard vault. The menu's BOSS RUSH starts a run made of nothing else. Both
hand rows lay themselves out for however many cards they hold, tightening their
pitch (and the fan its tilt and arc) so an eleven-relic row still clears the
deck and the three right-edge piles.

The browser entry imports only the engine facade and contains no DOM or canvas
host calls. The menu, game, help, history, and final outcome all render inside
one 960×600 logical canvas that scales to the viewport. Trick results remain on the
board, where color identifies the winning formation and any decisive kickers.
After the reveal flips, the winner's two cards fly out of their row and flank
the three wards, completing the five-card formation in the middle of the
board; a single golden halo then ignites around the whole set while the losing
pair ignites at its centre, chars through along a ragged ember front, and
crumbles into drifting ash. When play advances, those ashes gather at the
neutral pile and the two burned faces reform there instead of returning to
their old hand slots. Firebolt uses the same reappearance for the ward it
destroys. That halo remains gold
when the player wins; when the Arch Mage holds the wards, contracting crimson
seals, sinking ash, and encroaching shadow smother the formation instead. The
game's own vertex/pixel shaders keep both result fields moving off the live
frame clock even while the settled reveal itself holds still. A missing or
invalid defeat shader is treated as a visible fixture error rather than falling
back to an unshaded result. Black-school cards use a separate local-space
shader: a rotating violet accretion nexus collapsing into a black event horizon.

Cards never simply appear or vanish: every relic and ward is dealt off a
visible face-down pile beside the board — the opening hands, each round's
wards, and the two replacements both sides draw after a breach — and when a
breach's continue press ends the round, the resolved formation is scooped up:
its cards slide together into one pile, which then carries to a face-up
spent pile at the board's right edge, mirroring the draw pile on the left.
While the pile carries off, the three surviving cards in each hand slide down
into the slots the spent pair vacated, so the hands are settled before the
replacements are dealt beside them. A
won feint plays out in three beats: the winner's relics rise out of the hand,
the displaced wards are set aside at the board's edge, and both groups then
travel to their final slots, flipping face up or face down to match where
they land.

A feint is the only way a relic's identity crosses the table: both sides are
looking at the wards, so whichever hand takes one has gained a card the other
side has already read. Those slots are tracked and wear a small eye until the
relic is spent — on the player's fan the eye warns that the Arch Mage knows
that relic, and on the Arch Mage's sealed row it marks a relic the player is
owed a look at, so the seal there is only veiled: its field goes translucent
and the rank and school show through while the engraving stays crisp. Cards
drawn off the pile are secret again, and a ward on the board is public anyway,
so neither is ever marked.

Freeze Ray locks its targeted ward out of feints for both the player and the
Arch Mage. A ward frozen after the feint stays in its board slot when the
breach resolves while the other wards rotate, then thaws for the next round.
The freeze also thaws as soon as a feint resolves, so each cast affects only
the current transition.

The player's relics are held as an overlapping, tilted fan. Relics keep the
order they were dealt in until C or V regroups them by school or by rank —
the eye marks, cursor, and any pending binds follow their relics to the new
slots; the fan slides open around whichever one the cursor
holds, giving it room rather than lifting it in front of its neighbours. Parting
moves a relic along the fan's arc, so it tilts and dips as it goes instead of
skidding sideways out of the curve, and relics turn into and out of that tilt as
they are dealt or spent. The Arch Mage's sealed row and the board's wards stay
flat.

## Controls

The app boots to a menu screen with NEW RUN, BOSS RUSH, and HOW TO PLAY
options. Arrow keys (or hovering) move the selection, Enter/Space or a click on
an option activates it, and ? jumps straight to help. Activating NEW RUN or
BOSS RUSH picks a starting spell and then begins the run — that gesture also
unlocks browser audio — and M returns to the menu from the vault, abandoning
the run. The menu and the heist board are separate screen modules
(`menu.walu` and `game_screen.walu`); `main.walu` only decides which one
receives engine callbacks. One heist screen spans a whole run, dealing each
vault in place from the run state it owns.

Both screens stand on one city map. `main.walu` owns it (`city_map.walu` for
the model, `city_map_render.walu` for the drawing) and draws it under whichever
screen is live, because the run crosses it rather than visiting it: the title
drifts over a city, the starting spell is bought at the first street vendor on
the route, every fence between two vaults is the next vendor along it, and a
vault is the house the camera goes into. The route alternates the two — vendor,
house, vendor, house — with the road already walked drawn solid in gold behind
the robbers and the road still to come dashed ahead of them, so the map is the
run's progress bar as well as its backdrop.

The vault keeps its own backdrop, and the map goes into it rather than cutting
to it. A screen change is two movements taking the screen in turn, never at
once: the robbers walk the block to the door, and then the camera closes on the
house through five octaves of zoom until the room behind it is the whole frame.
The route furniture drops away first, then the surrounding city, so what is left
growing into the frame is the one house being entered — and its dark interior is
where the vault's backdrop fades in. Coming out runs the same movement
backwards: the room opens up into a block, a street, and a city, and only once
the map is standing still does the fence's offer list arrive on it. Choosing NEW
RUN pans from the drifting title view to the first vendor the same way.

Keyboard (in the vault):

- Arrow keys or WASD move focus; up/down switches between relics and wards.
- Space binds or unbinds the focused relic or ward.
- Enter commits a feint or breach.
- P passes during the feint phase.
- C sorts the relic fan by school (color); V sorts it by rank.
- 1 and 2 enter targeting for the spell on that key; arrows choose a ward, Enter
  casts, and Esc cancels. The other spell's key swings the aim over to it, and
  the aimed spell's own key calls the cast off.
- H opens the breach ledger, ? opens help, and R restarts the run.
- On the vault's verdict, Enter, Space, or a click takes whatever the run has
  next: the fence before the following vault, or the first vault of a fresh run.
- At the fence, arrows move between the offers, Enter or a click buys the one
  under the cursor, and Esc walks past all of them into the vault. R still
  restarts the run and M still returns to the menu.
- Enter, Space, or Esc skips a running deal or feint animation.

Mouse (Love2D-style engine callbacks in logical canvas coordinates):

- Hovering a relic or ward steers the same focus cursor the arrow keys move,
  so the fan parts around the card under the pointer; hovering dead space
  leaves the cursor where it was.
- Left-clicking a relic or ward binds or unbinds it. Hit tests respect the
  fan's tilt and parting, and prefer the topmost overlapping relic.
- On-screen capsules commit a breach or feint or pass the feint; the commit
  capsule only lights up while the pending binds would be accepted.
- A click advances or skips reveals and animations, closes the ledger and
  help, enters the next vault from the verdict screen, and the footer's
  "? HELP" opens help.

| File | Purpose |
| --- | --- |
| `project.js` | Stable source-project adapter for playground and conformance hosts. |
| `main.walu` | Thin engine adapter that owns the session and routes callbacks to the live screen. |
| `menu.walu` | The pre-game menu screen: presentation plus begin-gesture interpretation. |
| `city_map.walu` | DOM-free city generation, the alternating vendor/vault route, and the camera pans and dissolves that carry it between screens. |
| `city_map_render.walu` | WebGL2 primitive drawing for the city, its walked and upcoming route, and the colored street streak, all at one opacity. |
| `game_screen.walu` | The heist screen: rules/flow/choreography wiring and its input adapters. |
| `run.walu` | DOM-free run state: the vault sequence, its boss cadence, the spell loadout, and the mana carried between vaults. |
| `shop.walu` | DOM-free intermission between vaults: the two offers a visit stocks, what they charge, which are spent, and which the cursor holds. |
| `shop_render.walu` | Where the fence's offer list sits on the city map, at the map's own opacity, and the pointer's row lookup for it. |
| `game.walu` | DOM-free rules, AI, commands, outcomes, and read-only presentation view. |
| `flow.walu` | DOM-free input gating, focus, modal, selection, and reveal phase transitions. |
| `choreography.walu` | Domain-level deal, feint, breach, fan, pile, reveal timing, and animation choreography. |
| `render.walu` | Playfield/modal drawing behind a single nested frame interface. |
| `spell_cast.walu` | Target-aware spell trajectory and shared impact geometry. |
| `spell_launch*.walu` | Stable launch seam plus one independently editable carrier/impact module per spell. |
| `burn_particles.walu` | Shared card-burn shader binding and deterministic ash/ember primitives. |
| `effect_shaders.walu` | Data-driven effect registry and shared-vertex coordination. |
| `shader_program.walu` | Deep lifecycle module for one independently managed fragment program. |
| `shader-sources.js` | Convention-based fragment discovery shared by Vite and shader behavior tests. |
| `src/shaders/` | Shared vertex stage and independently reloadable effect fragment stages. |
| `presentation_resources.walu` | Asynchronous asset loading, GPU promotion, audio, effects, and disposal. |
| `snapshot.walu` | Shared validated snapshot primitives and atomic payload framing. |
| `test/game_fixture.walu` | Narrow mutable test adapter for deterministic rule arrangements. |
| `sim.test.walu` | Deterministic Vitest assertions for rules, flow, snapshots, and full-game completion. |
| `run.test.walu` | Deterministic Vitest assertions for the boss cadence, mana carryover, the carried loadout, and where a run ends. |
| `shop.test.walu` | Deterministic Vitest assertions for the fence's stock, prices, spent offers, and trades. |
| `city_map.test.walu` | Deterministic Vitest assertions for the generated streets, the route's alternating stops, and the pans and dissolves between screens. |
| `card.stories.walu` | Storybook stories for the relic: every state the board can put a card in, without dealing a heist that produces it. |
| `.storybook/main.js` | Storybook configuration: the story glob and the compiler options stories are built with. |
| `tests/game-driver.js` | Shared browser-test seam for booting a heist and observing rendered frames. |
| `tests/spell-effects.spec.js` | Spell-presentation behavior isolated from menu and gameplay browser coverage. |
| `waluau.assets.json` | Typed package manifest for the card back, vault font, and flip sound. |

Vite discovers every `src/shaders/*.frag` file through `shader-sources.js` and
maps it through the plugin's `shaderSources` option. Production bundles the same source contract; in
development, a fragment edit replaces only its live effect program and an edit
to the shared vertex stage refreshes every registered effect without rebuilding
the Wasm game. Tests discover the catalog rather than maintaining shader totals.
Invalid live edits keep the previous program allocated while reporting the
current shader diagnostic (fatal overlay for the defeat shroud, console warning
for optional effects), and a later valid edit clears that diagnostic.
Shader files are intentionally not runtime assets in `waluau.assets.json`.

The sealed-card artwork is the committed vector
[`assets/card-back.svg`](assets/card-back.svg). Until its asynchronous image
load and GPU copy complete—or if either reports a structured failure—the
renderer shows only a neutral sealed silhouette; it does not maintain a second
procedural copy of the artwork. Text uses the packaged Cinzel Bold font
(a static wght=700 instance of the Cinzel variable font, whose engraved
Trajan-style capitals match the tarot card artwork) after its FontFace
resource has been copied to a GPU glyph atlas, with the built-in bitmap font
as the not-ready/failure fallback. Source image/font resources are released
immediately after the GPU copies succeed; GPU resources retain their own
explicit lifetime.

Card turns use the packaged [`assets/card-flip.wav`](assets/card-flip.wav),
decoded through the engine's sound-effect service. Playback is triggered
shortly before each animated card crosses edge-on, compensating for browser and
device output latency. Because the effect is part of the intended
presentation, an undeclared, missing, undecodable, or
unplayable sound stops the fixture on a diagnostic canvas showing the asset
path, stable error code, and host message.
The opening deal waits at its first frame until a key gesture unlocks browser
audio, so no pre-gesture effect can be queued and released late.

Cinzel is distributed under the SIL Open Font License 1.1; the bundled
license is [`assets/OFL-Cinzel.txt`](assets/OFL-Cinzel.txt).

## Storybook

```bash
pnpm --filter ante storybook
```

The relic's states — face up, sealed, bound, focused, watched, mid-turn, one per
school, and in the fan — are stories in
[`src/card.stories.walu`](src/card.stories.walu), drawn by the same
`render.walu` the board draws them with and loading the same packaged card back,
font, and school shaders. Looking at one is therefore looking at the game's own
presentation, not a mock of it. `@waluau/storybook` is the framework; see its
README for how a story file is written.

## Building

```bash
cargo run -p waluau-cli -- fixtures/poker-tricks/main.walu \
  -o dist/arcane-heist.wasm --emit-js \
  --manifest fixtures/poker-tricks/waluau.assets.json
```

The distributable build copies all declared assets under `dist/assets/` with
content fingerprints. Generated sibling JavaScript maps the logical Waluau
paths (`assets/card-back.svg`, `assets/Cinzel-Bold.ttf`, and
`assets/card-flip.wav`) to those emitted URLs and carries their typed asset
kinds into the browser host.
