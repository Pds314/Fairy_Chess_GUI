# Fairy Chess GUI

A customizable chess GUI, renderer, and engine implemented in Rust. This project supports standard FIDE chess as well as arbitrary "Fairy Chess" variants through custom board and piece configurations.

## Features

* **Rule Enforcement**: Full support for standard chess mechanics including turn validation, check, checkmate, standard and non-standard en passant, castling, and promotions.
* **Draw Conditions**: Enforces the fifty-move rule, threefold repetition, and dead position (insufficient material) states.
* **Variant Support**: Define custom pieces, boards, and promotion zones via `.game` and `.pieces` configuration files. Piece movesets are determined by a custom notation syntax.
* **Dual Interface**: Play via the graphical interface (built with `iced`) or use the integrated terminal command interface for movement, analysis, and board state queries.
* **Engines & Tournaments**: Includes built-in basic engines with configurable "personalities" (e.g., positional biases, swarm factors). Features an automated tournament runner to test engines against one another.
* **Analysis & PGN**: Tools for evaluating positions, analyzing material and mobility disparities, and PGN export/import capabilities.

## Compilation

Simply compile the project using the cargo package manager and the rustc compiler with:
```bash
cargo build --release
```

and run the resulting executable in the `/target/release/` directory. 

Alternatively, you can build in the `/target/debug/` directory (with fewer compiler optimizations) by running:
```bash
cargo build
```

To temporarily build and run the project immediately:
```bash
cargo run
```

## Installing Cargo

If you do not have Cargo or Rustc, the recommended way to install them is to use Rustup: [https://rustup.rs](https://rustup.rs)

**Linux and Mac OS:**
```bash
curl --proto '=https' --tlsv1.2 -sSf [https://sh.rustup.rs](https://sh.rustup.rs) | sh
```

**Windows:**
Download the executable installer from the website for your specific CPU architecture.

## Configuration

The software relies on configuration files located in the `assets/` directory:

* **`.game` files**: Define the board dimensions, starting position (FEN-like format), and promotion zones.
* **`.pieces` files**: Define the rules for individual pieces. Piece logic is enforced via the syntax described in `NOTATION.md`.
* **`.personality` files**: Configure the evaluation weights and behaviors for the built-in AI opponents.

Custom textures can be applied by placing `.png` files in the `assets/pieces/` directory, following the naming convention `<color>_<texture_name>.png`.

## Terminal Interface

The Fairy Chess GUI includes a comprehensive terminal command interface located at the bottom of the GUI control panel. It allows you to control all aspects of the game via text commands. 

### Available Commands

**Movement & Game Control**
* `move <from> <to>` - Make a move using algebraic notation (e.g., `move e2 e4`)
* `undo` or `u` - Undo the last move
* `reset` or `r` - Reset board to starting position
* `turn` - Display whose turn it is

**Board Display**
* `board` or `b` - Pretty print the current board with coordinates
* `status` - Show comprehensive game status

**Analysis & Evaluation**
* `analyze` or `a` - Run comprehensive position analysis
* `moves` - Generate and display all legal moves
* `eval` - Evaluate current position with selected engine
* `best` - Make the best move according to the engine

**Engine Control**
* `engine` - Show current engine status
* `engine <type>` - Set evaluation engine (simple, random, pst, tactical, swarm)

**Help**
* `help` or `h` - Show complete command list

### Usage Example
```text
> move e2 e4
Attempting move: e2 -> e4
Move successful!
```

## License

This project is licensed under the MIT License - see the LICENSE file for details.
