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
breach win ends the heist, while player wins never end the five-breach run early,
leaving room to maximize the final spark score.

The browser entry imports only the engine facade and contains no DOM or canvas
host calls. The game, help, history, and final outcome all render inside one
960×600 logical canvas that scales to the viewport. Trick results remain on the
board, where color identifies the winning formation and any decisive kickers.
After the reveal flips, the winner's two cards fly out of their row and flank
the three wards, completing the five-card formation in the middle of the
board; a single golden halo then ignites around the whole set while the losing
pair fractures into suit-lit shards and disintegrates. That halo remains gold
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

- Arrow keys or WASD move focus; up/down switches between relics and wards.
- Space binds or unbinds the focused relic or ward.
- Enter commits a feint or breach.
- P passes during the feint phase.
- C sorts the relic fan by school (color); V sorts it by rank.
- H opens the breach ledger, ? opens help, and R restarts.
- Enter, Space, or Esc skips a running deal or feint animation.

| File | Purpose |
| --- | --- |
| `main.walu` | Engine entry point and keyboard-driven game flow. |
| `render.walu` | Playfield/modal rendering and the game's vertex/pixel effect shader. |
| `game.walu` | Host-independent deck, ranking, AI, and scoring rules. |
| `sim.walu` | Deterministic assertions for rankings and full-game completion. |
| `waluau.assets.json` | Typed package manifest for the card back and vault font. |

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

Cinzel is distributed under the SIL Open Font License 1.1; the bundled
license is [`assets/OFL-Cinzel.txt`](assets/OFL-Cinzel.txt).

## Building

```bash
cargo run -p waluau-cli -- fixtures/poker-tricks/main.walu \
  -o dist/arcane-heist.wasm --emit-js \
  --manifest fixtures/poker-tricks/waluau.assets.json
cargo run -p waluau-cli -- fixtures/poker-tricks/sim.walu -o sim.wasm
```

The distributable build copies both declared assets under `dist/assets/` with
content fingerprints. Generated sibling JavaScript maps the logical Waluau
paths (`assets/card-back.svg` and `assets/Cinzel-Bold.ttf`) to those
emitted URLs and carries their image/font types into the browser host.
