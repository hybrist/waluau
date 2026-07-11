# Poker Tricks fixture

A compact player-versus-computer trick-taking game written in Waluau. Each
player starts with five cards and 10 points. Three community cards are revealed
each round. Before playing the trick, either player may pass or exchange up to
three hand cards with the same number of community cards. The wager is one
point per selected card, so swapping one, two, or three cards wagers 1–3 points.
Only one proposal executes: the higher wager has priority, with equal wagers
decided by comparing the proposed boards as three-card poker hands. The
computer searches every legal exchange for an improved five-card hand.

After the single swap opportunity, the player chooses two cards and the
computer searches its hand for the best two. Those cards and the community
cards make ordinary five-card poker hands.

The winner earns points equal to the round number plus the complete wager pot,
so the six base trick values are 1 through 6 points. A tied trick refunds both
wagers. Played cards are replaced from the shared deck. Starting with ten cards
in hand and consuming seven cards per round uses all 52 cards exactly; swaps do
not consume cards. The higher point total then wins. A running history keeps
every completed trick visible, including swap priority and wagers, the board,
both played pairs, the resulting poker categories, winner, and points awarded.

| File | Purpose |
| --- | --- |
| `main.walu` | Browser entry point and interactive card-table UI. |
| `game.walu` | DOM-free deck, poker-hand ranking, computer choice, and scoring rules. |
| `sim.walu` | Deterministic assertions for rankings, computer play, draws, and game completion. |

## Building

```bash
# Browser entry (requires the playground or another DOM host):
cargo run -p waluau-cli -- fixtures/poker-tricks/main.walu -o poker-tricks.wasm

# Headless rules check:
cargo run -p waluau-cli -- fixtures/poker-tricks/sim.walu -o sim.wasm
```

## Presentation

The table stays DOM-rendered so every playable card remains a keyboard-accessible
button with a stable label. Layered rank, suit, and pip elements give the cards a
playing-card face; custom CSS supplies the felt table, patterned card backs,
hover/focus/selection feedback, and the staggered 3D flip at showdown. The reveal
animation disables itself when the browser requests reduced motion.
