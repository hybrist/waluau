# Arcane Heist fixture

A six-breach, player-versus-Arch-Mage trick-taking game built on the Waluau 2D
game engine. It preserves Poker Tricks' deck, exchange, wager, hand-ranking,
computer search, and scoring rules while replacing the casino-table presentation
with a magical robbery inside an arcane vault.

The four suits are now the Red, Blue, Black, and Green schools of magic. Cards
are relics, the shared board is the vault's wards, points are sparks, exchanges
are feints, and each trick is a breach. Poker categories are presented as magical
formations such as a bound pair, arcane sequence, and perfect convergence.

The browser entry imports only the engine facade and contains no DOM or canvas
host calls. The game, help, reveal, history, and final outcome all render inside
one 960×600 logical canvas that scales to the viewport.

## Controls

- Arrow keys or WASD move focus; up/down switches between relics and wards.
- Space binds or unbinds the focused relic or ward.
- Enter commits a feint or breach.
- P passes during the feint phase.
- H opens the breach ledger, ? opens help, and R restarts.

| File | Purpose |
| --- | --- |
| `main.walu` | Engine entry point and keyboard-driven game flow. |
| `render.walu` | Platform-independent playfield and modal rendering. |
| `game.walu` | Host-independent deck, ranking, AI, and scoring rules. |
| `sim.walu` | Deterministic assertions for rankings and full-game completion. |

## Building

```bash
cargo run -p waluau-cli -- fixtures/poker-tricks/main.walu -o arcane-heist.wasm
cargo run -p waluau-cli -- fixtures/poker-tricks/sim.walu -o sim.wasm
```
