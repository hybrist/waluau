# Ante Magic fixture

A player-versus-Arch-Mage trick-taking game built on the Waluau 2D game engine.
It preserves Poker Tricks' deck, exchange, wager, hand-ranking, and computer
search while replacing the casino-table presentation with a magical robbery
inside an arcane vault. A won breach pays its round and pot, plus a bonus that
climbs with the rarity of its five-card poker hand.

The four suits are now Red Cups, Blue Bells, Black Swords, and Green Leaves,
the four schools of magic. Cards
are relics, the shared board is the vault's wards, points are sparks, exchanges
are feints, and each trick is a breach. Poker categories are presented as magical
formations such as a bound pair, arcane sequence, and perfect convergence.
The Arch Mage can commit any valid feint without a mana balance. Winning a
breach clears the vault; losing one costs a heart and starts another breach in
the same vault, unless it was the run's last heart.
Each breach has exactly three feint opportunities. The first follows a
three-card community deal, then a fourth card and a fifth card are dealt before
the second and third opportunities. Both sides propose once per opportunity;
the stronger proposal executes, and every non-pass proposal must use strictly
more cards than either side's previous maximum. The ceiling is the current
community size, so a three-card flop swap can still be followed by swaps of four
and five. In a boss battle, proposal history resets for the next breach.

After the river feint, each side commits two private cards and the strongest
five-card poker hand among those two and the five community cards wins. As in
Texas Hold'em, the winning five may use both, one, or neither committed card.

A vault is one heist inside a longer roguelike run of nine, and clearing one
costs its ante: the vault takes a cut of the mana the robbers are holding, and
what is left is what the next vault is dealt with. The pool is the run's, not
the vault's, and every cast spends from it for good. Between two vaults the run
stops at the shop, where that same mana buys spell scrolls.

A run also starts with three hearts. Every hand the Arch Mage wins removes one,
including a hand inside a vault the player ultimately clears. Hearts travel
through the fence and into later vaults without refilling; losing the last one
ends the run as soon as that hand resolves.

The ante climbs, and it climbs past what a typical vault pays. Early vaults ask
for well under it, so a run that is going well banks a surplus; the last act
asks for more than ordinary hands earn, so the third act is spent out of the
pile the first two banked and the ninth vault is reached on fumes. A rare hand
can beat the curve with a windfall. That is the ramp: not a breach that is
harder to win, but a win that has to be worth more.

There are two ways a run ends. Losing the last heart ends it immediately.
Clearing a vault and then not holding its ante ends it too. In either case the
next heist is the first vault of a fresh run. R restarts the run from vault one
at any point, which is also why a settled vault is never left standing on the
board.

Clearing the ninth vault wins the run — and does not stop it. The city goes on,
the table runs out, and the ante takes over on a formula whose step grows every
vault, so an endless run walks on out of whatever surplus it arrived with and no
further.

The robbers set out with the one spell the menu picked and can hold two ready at
once, on keys 1 and 2. A visit to the shop stocks exactly two spell scrolls,
drawn without replacement. A scroll for a known spell carries its next level;
a scroll for a new kind starts at level one and takes the next free key. There
is no separate learn-or-upgrade product and no trade prompt: once both keys are
taken, a scroll for another kind is unavailable rather than forgetting a spell
the run already carries. Every scroll costs four mana. Buying one marks that
line sold and nothing takes its place; reaching the next shop is what restocks
the offers. Losing the last heart ends the run and everything it bought with
it, so a fresh run sets out with its starting spell at level one again.

Every third vault of a run is a boss battle: both sides hold seven relics rather
than five, widening every feint and committed-pair choice. Spent pairs are
replaced after each breach so the seven-card choice remains the challenge until
the vault is cleared or the run runs out of hearts. Everything else — feints,
wagers, and scoring — matches a standard vault. The menu's BOSS RUSH starts a
run made of nothing else: five vaults rather than nine, on its own steeper ante.
Both hand rows tighten their pitch (and the fan its tilt and arc) so a
seven-relic row still clears the deck and the three right-edge piles.

The browser entry imports only the engine facade and contains no DOM or canvas
host calls. The menu, game, help, history, and final outcome all use the live
canvas size. A uniform unit scale keeps cards, text, circles, and line widths
proportional, while the logical viewport expands along whichever canvas axis
has more room. Semantic regions—headings, hand rows, wards, controls, piles,
and footers—place themselves from those live dimensions, so additional width
or height becomes game space rather than a fixed board or letterbox. Trick
results remain on the board, where
color identifies the winning formation and any decisive kickers.
After the reveal flips, the winner's two committed cards fly out of their row
and flank the five community cards. The best five are marked within that
seven-card pool; a single golden halo then ignites around the whole set while the losing
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

Relics use a 92-by-128 cut-paper silhouette, close to the traditional playing-
card ratio, with rounded corners and no printed perimeter stroke. Their dark
parchment carries rubbed washes and fine fibers rather than a perfectly even
UI fill. Number cards arrange one small school glyph per rank in celestial
constellations — a seven visibly carries seven glyphs — while J, Q, and K keep
their familiar ranks as a hooded adept, orb-bearing seer, and crowned
hierophant. The Ace is a single major omen. This keeps the deck mechanically
legible without borrowing the mirrored portraits or suit layout of a poker
deck.

Cards never simply appear or vanish: every relic and ward is dealt off a
visible face-down pile beside the board — the opening hands, each round's
flop, turn, river, and the two replacements both sides draw after a breach — and when a
breach's continue press ends the round, the resolved formation is scooped up:
its cards slide together into one pile, which then carries to a face-up
spent pile at the board's right edge, mirroring the draw pile on the left.
While the pile carries off, the surviving cards in each hand slide down
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
Arch Mage. In a boss battle, a ward frozen after the feint stays in its board
slot when the breach resolves while the other wards rotate, then thaws for the next round.
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
  next: the fence before the following vault, the walk on into the endless city
  after the ninth, or the first vault of a fresh run after either kind of loss.
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
  help, and enters the next vault from the verdict screen.

| File | Purpose |
| --- | --- |
| `project.js` | Stable source-project adapter for playground and conformance hosts. |
| `main.walu` | Thin engine adapter that owns the session and routes callbacks to the live screen. |
| `menu.walu` | The pre-game menu screen: presentation plus begin-gesture interpretation. |
| `city_map.walu` | DOM-free city generation, the alternating vendor/vault route, and the camera pans and dissolves that carry it between screens. |
| `city_map_render.walu` | WebGL2 primitive drawing for the city, its walked and upcoming route, the last authored vault's landmark house, and the colored street streak, all at one opacity. |
| `game_screen.walu` | The heist screen: rules/flow/choreography wiring and its input adapters. |
| `run.walu` | DOM-free run state: the vault sequence, its boss cadence and ante table, the finish line and endless tail, the spell loadout, and the mana and hearts carried between vaults. |
| `shop.walu` | DOM-free intermission between vaults: behavior-bearing item stock, quoted prices, spent offers, cursor input, and hot-replacement snapshots. |
| `items/` | The common item behavior/snapshot seam, its restore catalog, and concrete spell scrolls that teach or raise a spell without replacing the loadout. |
| `game.walu` | DOM-free rules, AI, commands, outcomes, and presentation snapshots. |
| `flow.walu` | DOM-free input gating, focus, modal, selection, and reveal phase transitions. |
| `choreography.walu` | Domain-level deal, feint, breach, fan, pile, reveal timing, and animation choreography. |
| `ui/layout.walu` | Retained, DOM-free intrinsic layout storage: stable boxes, in-place measurement/arrangement, rows, columns, padding, alignment, flex, and retained bounds. |
| `ui/node.walu` | The opaque retained-node interface shared by visible things: controlled presentation, measure, arrange, paint, and paint-order hit traversal. |
| `ui/interaction.walu` | Pointer enter/leave/down/up/click routing plus focus scopes, directional navigation, activation, and modal focus. |
| `ui/story.walu` | The retained-story boundary: load the presentation surface and place one stable node at the centre of a flex stage. |
| `entities/` | One file per thing on the board — see the entity table below. |
| `ink.walu` | The drawing vocabulary entities share: type, panels, school colours, and the one fade a screen is taken down by. |
| `plate.walu` | The material the vault is built from: lit faces, etched borders, domes, wells, halos, and the ramps they are made of. |
| `easing.walu` | The four curves the board moves on. |
| `card_burn.walu` | How a card comes apart: the captured sheet, the advancing front, and the ash that peels off it. |
| `render.walu` | The board, composed: which entities are on it this frame, which band each gets, and the cards in flight between them. |
| `spell_cast.walu` | Target-aware spell trajectory and shared impact geometry. |
| `spell_launch*.walu` | Stable launch seam plus one independently editable carrier/impact module per spell. |
| `burn_particles.walu` | Shared card-burn shader binding and deterministic ash/ember primitives. |
| `effect_shaders.walu` | Data-driven effect registry and shared-vertex coordination. |
| `shader_program.walu` | Deep lifecycle module for one independently managed fragment program. |
| `shader-sources.js` | Convention-based fragment discovery shared by Vite and shader behavior tests. |
| `src/shaders/` | Shared vertex stage and independently reloadable effect fragment stages. |
| `presentation_resources.walu` | Asynchronous asset loading, GPU promotion, audio, effects, and disposal. |
| `test/game_fixture.walu` | Narrow mutable test adapter for deterministic rule arrangements. |
| `sim.test.walu` | Deterministic Vitest assertions for rules, flow, snapshots, and full-game completion. |
| `run.test.walu` | Deterministic Vitest assertions for the boss cadence, ante table and endless tail, persistent hearts, mana carryover, the carried loadout, and run outcomes. |
| `economy.test.walu` | Aggregate Vitest measurements of what a vault pays and how a run ends, played from a shuffled deck by a reference policy — the numbers the ante table is priced against. |
| `shop.test.walu` | Deterministic Vitest assertions for item stock, spending, spell levels, full-loadout refusal, lifecycle, and snapshots. |
| `city_map.test.walu` | Deterministic Vitest assertions for the generated streets, the route's alternating stops, and the pans and dissolves between screens. |
| `ui/node.test.walu` | Headless assertions for retained identity, layout, arrangement, hit order, and controlled presentation. |
| `ui/presentation.test.walu` | Headless assertions for composition and the type-sized retained presenters. |
| `card.stories.walu` | Storybook stories for the relic: every state the board can put a card in, without dealing a heist that produces it. |
| `hand.stories.walu` | Storybook controls for the live hand-fan entity across card counts, selections, and focus positions. |
| `entities/shop.stories.walu` | Storybook states and an interactive session for the retained shop entity. |
| `ui/layout.stories.walu` | Storybook stories for the retained layout solver itself, on synthetic leaves. |
| `.storybook/main.js` | Storybook configuration: the story glob and the compiler options stories are built with. |
| `tests/game-driver.js` | Shared browser-test seam for booting a heist and observing rendered frames. |
| `tests/spell-effects.spec.js` | Spell-presentation behavior isolated from menu and gameplay browser coverage. |
| `waluau.assets.json` | Typed package manifest for the card back, vault font, and flip sound. |

## Materials

Ante is lit from one direction. Everything drawn by the game — a chip, a modal
page, an ability socket, a rooftop on the city map — is a surface under that
light rather than an outline of one, and the vocabulary for saying so lives in
[`src/plate.walu`](src/plate.walu).

A **plate** is the shape the chrome is cut from: a face carrying a vertical
gradient, a hairline highlight along its top inside edge and a shadow along its
bottom, and a border drawn as four rails so the lit side is brighter than the
shaded one. A page-sized plate also carries corner brackets. `ink.panel` is a
plate, so every panel in the game is one without asking.

Curved surfaces are stacks of circles whose centres drift against the light,
which is what makes them directional: **dome** for a cap that catches the light
at its top left, **well** for a socket cut into a plate whose lit crescent
collects along its lower wall. **halo** and **aura** put a colour into the space
around a shape, which is how the game says ready, reached, or selected.

Controls are the same idea one level up. `entities/button.walu` holds a **Look**
— the finish a control is cut in, owned by its caller rather than built per
frame — a **Mood**, which is what the screen thinks of the control, and a
**Touch**, which is the separate question of what the pointer is doing to it.
The capsule, the standing chips, the menu's rows and the shop's rows are all
presets over it, so a control that gains a state gains it everywhere. Under the
pointer a face lifts; under a press it sinks, drawn by moving the bevel rather
than by darkening, because on plates this dark there is little light to take
away.

Two ramps underneath all of it: `shaded_rectangle` names a brightness at each of
the four corners, and `veil_rectangle` names an opacity instead. The difference
matters — a shade ramp darkens what it draws, where a veil lets what is already
underneath show through, so a vignette or a shadow that thins out has to be the
second one.

## Retained UI

Everything visible on the board owns a stable opaque `ui.Node`. Constructors run
when a screen or bounded presenter is created; explicit `present` methods update
the values those nodes show. Each frame remeasures, rearranges, and paints the
existing tree. Painting is intentionally still immediate and complete, but a
settled frame does not rebuild node trees, layout trees, rectangles, or solver
scratch.

The small interface lives in [`src/ui/node.walu`](src/ui/node.walu). Nodes
compose into retained rows, columns, layers, padding, and minimum-size wrappers;
[`src/ui/layout.walu`](src/ui/layout.walu) stores their intrinsic measurements,
arranged bounds, baselines, overflow result, and flex scratch in place. All
layout-affecting mutation goes through presentation methods, leaving one clear
place to add dirty propagation and cached layout decisions later without
changing callers. There is no reactive dependency graph, and pre-rendered
subtree caching is not part of this design.

Measuring needs only optional graphics metrics, so the whole tree remains
DOM-free and can be tested headlessly. With a live context a label asks the
loaded font for its width; without one it uses the built-in bitmap-font advance.
Drawing receives the WebGL2 presentation surface and the bounds retained by
layout.

[`src/ui/interaction.walu`](src/ui/interaction.walu) routes pointer movement,
enter/leave, down/up, and release-synthesized clicks through reverse paint-order
hit traversal. The same retained bounds feed focus scopes, arrow navigation,
and Enter/Space activation. Focus is generic UI state; card selection, spell
targets, purchase rules, and other game choices remain domain state. Modal focus
scopes prevent the covered board from responding.

| Entity | What it is |
| --- | --- |
| `entities/card.walu` | The relic: rank, school, and the four readings the table can have of it. The atom every other board entity is built from. |
| `entities/card_row.walu` | Cards laid flat on a pitch: the sealed row, the wards, and the five-card formation a settled breach leaves standing. |
| `entities/hand_fan.walu` | The player's hand as a tilted, overlapping arc, and the hit test that undoes its rotation. |
| `entities/ward_panel.walu` | The dark field the wards stand on, sized from the row it backs. |
| `entities/powered_card.walu` | A ward under a spell: Firebolt's burn, Freeze Ray's shell, Raise Card's grave, Growth's division. |
| `entities/deck.walu` | The sealed draw pile and its count. |
| `entities/pile.walu` | One of the three right-edge piles, face up on its last card. |
| `entities/hud.walu` | The duel title as a cartouche cut into the header band. |
| `entities/footer.walu` | The centred way back to the menu and the lower-right ability diamond. |
| `entities/vitals.walu` | The run's hearts as pips and its pool as a struck coin, in the board's lower-left corner. |
| `entities/ability_diamond.walu` | Four ready-ability sockets in South, West, North, East face-button order. |
| `entities/button.walu` | The control every clickable rectangle is cut from: one finish, three moods, and what the pointer is doing to it. |
| `entities/capsule.walu` | A clickable control, as wide as its own label. |
| `entities/action_bar.walu` | The band that asks for a decision, and the hit test for the controls it asked with. |
| `entities/label.walu` | A line of type that measures itself against the font that is loaded. |
| `entities/modal.walu` | A titled page laid over the vault, scaled to fit whatever canvas it is on. |
| `entities/help_card.walu` | The help page: the job, the four schools, the two phases, hearts, and every control. |
| `entities/ledger.walu` | The breach ledger, one row per played hand. |
| `entities/verdict.walu` | The verdict of a fought vault, read as a moment in the run. |
| `entities/school_tile.walu` | One school of magic as a swatch, running the card's own field. |
| `entities/backdrop.walu` | The vault behind everything. |

Vite discovers every `src/shaders/*.frag` file through `shader-sources.js` and
maps it through the plugin's `shaderSources` option. Production bundles the same source contract; in
development, a fragment edit replaces only its live effect program and an edit
to the shared vertex stage refreshes every registered effect without rebuilding
the Wasm game. Tests discover the catalog rather than maintaining shader totals.
Invalid live edits keep the previous program allocated while reporting the
current shader diagnostic (fatal overlay for the defeat shroud, console warning
for optional effects), and a later valid edit clears that diagnostic.
Shader files are intentionally not runtime assets in `waluau.assets.json`.

The sealed-card artwork is the committed high-DPI
[`assets/card-back.png`](assets/card-back.png), authored in the card's 92×128
design space at 8× density (736×1024) with authentic linen card stock texture
and gold foil embossing so scaled cards remain crisp on high-DPI canvases. Until
its asynchronous image load and GPU copy complete—or if either
reports a structured failure—the
renderer shows only a neutral sealed silhouette; it does not maintain a second
procedural copy of the artwork. Text uses the packaged Cinzel Bold font
(a static wght=700 instance of the Cinzel variable font, whose engraved
Trajan-style capitals match the tarot card artwork) after its FontFace
resource has been copied to a GPU glyph atlas, with the built-in bitmap font
as the not-ready/failure fallback. The manifest-generated bundle owns decoded
image, font, and sound sources together. Image and font leases close after GPU
promotion, the sound lease remains live for playback, and bundle disposal is
idempotent across all three; the presentation state explicitly releases the
promoted GPU resources.

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

The deployed game carries its storybook with it, at `/storybook`:
`pnpm --filter ante build-storybook` builds it into `dist/storybook`, and the
Vercel build runs it after the game's own `vite build` so one deployment serves
both.

Every Ante story supplies a retained `ui.Node` to
[`src/ui/story.walu`](src/ui/story.walu). The story module loads the
same presentation surface as the game and makes the Storybook canvas a centred
flex stage. It constructs the subject and stage once; controls call presentation
methods before each draw, and the node's intrinsic measurement and flex factors
decide how it uses the available canvas.

The relic's states — face up, sealed, selected, focused, watched, mid-turn, one
per color, and in the fan — therefore render the real card, card-row, and
hand-fan entities from [`src/card.stories.walu`](src/card.stories.walu) and
[`src/hand.stories.walu`](src/hand.stories.walu), with the same packaged card
back, font, and color shaders as the board. Looking at one is looking at the
game's presentation, not a mock of it. `@waluau/storybook` is the framework;
see its README for the host-side story declaration contract.

## Building

```bash
cargo run -p waluau-cli -- fixtures/poker-tricks/main.walu \
  -o dist/ante-magic.wasm --emit-js \
  --manifest fixtures/poker-tricks/waluau.assets.json
```

The distributable build copies all declared assets under `dist/assets/` with
content fingerprints. Generated sibling JavaScript maps the logical Waluau
paths (`assets/card-back.png`, `assets/Cinzel-Bold.ttf`, and
`assets/card-flip.wav`) to those emitted URLs and carries their typed asset
kinds into the browser host.
