# Fairy Chess Notation Guide

Piece rules in this engine are defined using a custom text syntax located in `.pieces` configuration files. Whitespace is ignored everywhere except in names.

## Piece Configuration Format

Each piece is defined on a single line terminated by a semicolon (`;`). The line is split into exactly 5 sections separated by forward slashes (`/`):

1. **Display Name**: A name to use for display purposes (e.g., in the GUI or analysis).
2. **Texture Names**: A comma-separated list of texture filenames to look for in `assets/pieces/` to render that piece. Priority is top-to-bottom/left-to-right.
3. **Characters**: A character or short string used to identify the piece in PGN notation, FEN strings, or terminal rendering.
4. **Moveset**: The core movement logic string (detailed below).
5. **Properties**: Global boolean properties for the piece.

> **Example:**
> `Knight / knight, horse / N / +x / p;`

---

## The Moveset String (Section 4)

A moveset is a string defining how a piece moves. The `,` character indicates that a piece has one or more additional move patterns (e.g., `+*,x*` defines a Queen by combining Rook and Bishop patterns). 

Each pattern is composed of sequential "steps". Steps are built using directional prefixes, a base shape, step modifiers, and overall pattern modifiers.

### Base Shapes
* `+` : Orthogonal movement (up, down, left, right).
* `x` : Diagonal movement.
* `?` : A special "null" or "pass" move. Can only be used by itself (e.g., `?`) to allow a piece to skip its turn.

### Directional & Modifier Prefixes (Step-Level)
These prefixes apply to the immediate step and restrict or modify its vector. They combine additively.
* `^` : Forward
* `v` : Backward
* `<` : Left
* `>` : Right
* `-` : Subtractive modifier. Removes instead of adds a direction. (e.g., `-v+` means all orthogonal directions *except* backward).
* `=` : Direction lock. Forces the step to continue in the exact same vector as the previous step.
* `Number` : Limits the exact step distance in that direction. (e.g., `2+` moves exactly two squares orthogonally).

**Chaining Submoves:**
You can chain steps to create complex paths.
* `^+x` : Moves one square forward orthogonally, then one square along any outward diagonal (the two forward-most Knight moves).
* `^x+` : Moves diagonally forward, then outward orthogonally (the four forward Knight moves).
* `2+x` : Moves in a 3x1 "L" shape (like a Camel).

---

### Step Suffixes (Modifiers)
These suffixes are applied immediately after a step and dictate how that specific step behaves in sequence.

**Repetition & Stopping:**
* `*` : The step may optionally be repeated in the same direction across valid squares (e.g., `x*` is a sliding Bishop).
* `*N` : A number after `*` restricts the maximum repetitions (e.g., `^+*2` means move one or two steps straight forward).
* `?` : Optional stop. The piece may optionally stop at this step instead of continuing the chain.
* `#` : Resets the "center" of what counts as an outward move for the next sub-step.

**Ghost Markers (Transit Aliases):**
A ghost marker leaves an invisible "ghost" on the square the step **departed from**. This ghost points to the piece's final destination. Royalty projection is derived from the piece, never the ghost. Markers compose (e.g., `E&`).
* `e` : **Restricted Capture Alias** (`CAPTURE_EP`). Only movers with the `~` trait may capture the piece by landing on this departed square (En Passant).
* `E` : **Open Capture Alias** (`CAPTURE_OPEN`). Any capture-capable mover may capture the piece by landing on this square.
* `&` : **Castle Target** (`CASTLE_TARGET`). A castling partner (Rook) may land on this departed square. This is required to assert castling transit validity.
* `'` : **Bare Transit Ghost**. Has no capture behavior of its own, but projects royalty if its owner is royal. Used to enforce "cannot castle through check".

**Flight Captures (Capturing in passing):**
Allows a piece to capture pieces it passes over without stopping.
* `%` or `%!` : Captures enemy pieces in flight.
* `%@` : Captures friendly pieces in flight.

**Step Pass Permissions:**
By default, intermediate steps require empty squares. You can override what a piece is allowed to pass through or land on during a step:
* `_` : Can pass through empty squares.
* `!` : Can pass through enemy pieces.
* `@` : Can pass through friendly pieces.
*(Note: If a step has `*`, you can specify pass permissions specifically for the repetitions by appending them after the `*`, e.g., `*!`).*

---

### Pattern-Level Suffixes
These suffixes are applied at the very end of a move pattern and govern the final landing rules and overall behavior of the move.

**Final Landing Permissions:**
If unspecified, a pattern can land on empty squares or enemy pieces. Adding any of these overrides the default:
* `_` : Can land on empty squares.
* `!` : Can land on enemy pieces.
* `@` : Can land on friendly pieces.
* `!{Piece}` : **Capture Filter**. Suffixing `!` with a piece's exact character or name in braces (e.g., `!{K}`) means this pattern can *only* capture that specific piece type.

**Special Mechanics:**
* `u` : Requires the piece to be unmoved.
* `i` : Irreversible. Moving with this pattern resets the fifty-move rule.
* `~` : This move can capture *en passant* (allows capturing `e` ghosts).
* `o` : King-side castle. Used in conjunction with `&` ghosts to validate partner transits.
* `O` : Rook-side castle.

**Zones:**
You can restrict a pattern based on board zones defined in the `.game` file.
* `[zone_name]` at the **start** of a pattern requires the piece to depart from that zone.
* `[zone_name]` at the **end** of a pattern requires the piece to land in that zone.

---

## Piece Properties (Section 5)

Properties define the high-level game logic associated with the piece. They are defined as a `/` separated string of characters.

* `R` (Royal): This piece cannot move into check, and checkmating it ends the game.
* `r` (Royalty target): These pieces are collectively protected. Capturing the *last* instance of an `r` piece ends the game.
* `P` (Promoter): This piece may promote when entering a defined promotion zone.
* `p` (Promotion target): This piece is allowed to be promoted into.

---

## Standard Pieces Example

This is how standard Chess pieces are mapped using the DSL, taking full advantage of the ghost framework and irreversibility rules:

```text
Knight : Knight / knight, horse / N / +x / p;
Rook   : Rook / rook, castle, tower / R / +*, +*Ou / p;
Bishop : Bishop / bishop, elephant / B / x* / p;
King   : King / king, mann / K / +, x, <>+E_<>+E_ou / R;
Queen  : Queen / queen, lady / Q / +*, x* / p;
Pawn   : Pawn / pawn, soldier / P / ^x!~i, ^+_i, ^+_^+e_ui / P;
