# Poker Tricks fixture

A compact player-versus-computer trick-taking game written in Waluau. Each
player starts with five cards. Three community cards are revealed each round,
then the player chooses two cards and the computer searches its hand for the
best two. Those cards and the community cards make ordinary five-card poker
hands.

The winner earns points equal to the round number, so the six tricks are worth
1 through 6 points. Played cards are replaced from the shared deck. Starting
with ten cards in hand and consuming seven cards per round uses all 52 cards
exactly; the higher point total then wins. A running history keeps every
completed trick visible, including the board, both played pairs, the resulting
poker categories, winner, and points awarded.

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
