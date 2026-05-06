# Fairy Chess Notation Guide

Piece rules in this engine are defined using a custom text syntax located in `.pieces` configuration files. Whitespace is ignored everywhere except in names.

## Piece Configuration Format
Each piece is defined on a single line terminated by a semicolon (`;`). The line is split into 5 sections separated by forward slashes (`/`):

1. **Display Name**: A name to use for display purposes if needed.
2. **Texture Names**: A comma-separated list of texture filenames to look for in `assets/pieces/` to render that piece. Priority is top-to-bottom/left-to-right.
3. **Characters**: A character or short string used to identify the piece in case no texture is available or for terminal rendering.
4. **Moveset**: The core movement logic string (detailed below).
5. **Properties**: Global boolean properties for the piece.

### Example
```text
Knight / knight, horse / N / +x / p;
```

---

## 1. The Moveset String (Section 4)

A moveset is a string defining how a piece moves. The `,` character indicates that a piece has one or more additional move patterns. For example, `+*,x*` is a queen move; the comma terminates the existing Rook (`+*`) pattern and starts the Bishop (`x*`) pattern.

Each pattern is composed of "steps", which are built using directional prefixes, a base shape, and modifying suffixes.

### Base Shapes
* `+` : All outward orthogonal submoves.
* `x` : All outward diagonal submoves.

### Directional Prefixes (Restrictors)
These prefixes combine additively to restrict the direction of a submove.
* `^` : Forward
* `v` : Backward
* `<` : Left
* `>` : Right
* `-` : Subtractive modifier. Removes instead of adds a direction. For example, `-v+` means all orthogonal directions *except* backward.
* `Number` : A number prefixing a shape specifies the exact step distance in that direction. For example, `2+*` would be a sliding piece that only goes even distances. `2+` is exactly two squares orthogonally.

**Chaining Submoves:**
You can chain modifiers to create complex paths.
* `^+x` : Moves a square forward, and then along any outward diagonal. This allows the two most forward knight moves.
* `^x+` : Moves diagonally forward, and then outward orthogonally. This allows the four forward knight moves.
* `2+x` or `x2+` : Moves in a 3x1 "L" shape (like a Camel) instead of the standard 2x1 Knight move.

### Suffixes (Modifiers)
Suffixes affect the move up to that point.

**Repetition & Stopping:**
* `*` : The move may optionally be repeated in the same direction across empty squares (e.g., `x*` is a Bishop).
* `*N` : A number after `*` restricts the maximum repetitions (e.g., `^+*2` means a move restricted to one or two steps straight forward).
* `?` : Optional stop. The piece may optionally stop here instead of doing any more submoves (passing its turn if it stops on its own square, e.g., `?+`).
* `#` : Resets the "center" of what counts as an outward move for the next submove.
* `=` : Direction lock. Forces the step to continue in the exact same vector as the previous step.

**Permissions (Collisions):**
By default, non-terminal submoves require empty squares (`_!@`), non-terminal repetitions require empty squares (`_`), and the move overall can land on empty squares or enemy pieces (`_!`). You can override this:
* `_` : Can land on/go through empty squares.
* `!` : Can land on/go through enemy pieces.
* `@` : Can land on/go through friendly pieces.

*Note: Suffixing `!` with a specific piece character in braces (e.g., `!{K}`) acts as a **Capture Filter**, allowing the piece to only capture that specific piece type.*

**Special Mechanics:**
* `~` : This move captures *en passant*.
* `e` : The piece may be captured here *en passant* if the attacker can capture en passant.
* `E` : The piece can be captured here by *en passant* regardless of whether the attacker normally has the en passant trait.
* `u` : Unmoved requirement. This move can only be made by an unmoved piece.
* `i` : Irreversible. This move resets the fifty-move rule without needing to capture.
* `o` : King-side castling move. Creates intermediate squares where the rook can move to.
* `O` : Rook-side castling move. Moves to an available castling square simultaneously with the king.
* `&` : Castling partner landing marker. Specifies the exact square the partner piece (Rook) must land on during a castling maneuver.

**Zones:**
You can restrict a pattern to only trigger if starting in, or landing in, a defined board zone.
* `[zone_name]` at the **start** of a pattern requires the piece to start in that zone.
* `[zone_name]` at the **end** of a pattern requires the piece to land in that zone.

---

## 2. Piece Properties (Section 5)

Properties define the high-level game logic associated with the piece. They are defined as a `/` separated list.

* `R` (Royal): Cannot move into check. Checkmating any piece with this property ends the game.
* `r` (Royalty target): Capturing all instances of pieces with this property ends the game.
* `P` (Promoter): This piece may promote.
* `p` (Promotion target): This piece is allowed to be promoted to.

---

## Examples of Standard Pieces
```text
Knight : Knight / knight, horse / N / +x / p;
Rook   : Rook / rook, castle, tower, wazir, fortress / R, WW / +*, +*Ou / p;
Bishop : Bishop / bishop, elephant, ferz / B, FF / x* / p;
King   : King / king, mann, man, crown, lord, commoner / K, WF, M / +,x,<>+E_<>+E_ou / R;
Queen  : Queen / queen, lady, crown, ferz, princess, empress / Q, KK, WWFF / +*,x* / p;
Pawn   : Pawn / pawn, soldier, man, commoner / P / ^x!~,^+_,^+_^+e_u / P;
```
