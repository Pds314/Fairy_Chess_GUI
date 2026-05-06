# Terminal Command Interface

## Overview
The Fairy Chess GUI includes a comprehensive terminal command interface that allows you to control all aspects of the game via text commands. This provides a powerful alternative to the GUI for advanced users, enables scripting possibilities, and allows for deep engine configuration.

## Available Commands

### Movement & Game Control
* `move <from> <to>` or `m <from> <to>` - Make a move using algebraic notation (e.g., `move e2 e4`).
* `undo` or `u` - Undo the last move.
* `redo` - Redo an undone move.
* `reset` or `r` - Reset the board to the starting position.
* `turn` - Display whose turn it currently is.

### Board & Game Status
* `board` or `b` - Pretty print the current board state with coordinate labels.
* `status` - Show comprehensive game status (turn, move counters, game results).
* `pgn` or `p` - Print the current game in standard PGN format to the console.
* `loadpgn <text|file>` - Load a game from PGN text wrapped in quotes, or provide a path to a `.pgn` file (e.g., `loadpgn "1. e4 e5"`).

### Analysis & Evaluation
* `analyze` or `a` - Run a comprehensive position analysis.
* `moves` - Generate and display all legal moves for the current position.
* `eval` - Evaluate the current position using the selected evaluation engine.
* `best` - Force the currently active engine to make its best move immediately.

### Engine Configuration
The terminal allows you to configure White (`w`), Black (`b`), or the background Evaluation engine (`e`).

* `engine` - Show current engine status for all slots.
* `engine <type>` - Set the evaluation engine type (e.g., `simple`, `random`, `pst`, `tactical`, `swarm`).
* `depth <w|b|e> <n>` - Set the search depth for a specific engine (e.g., `depth w 6` sets White's depth to 6).
* `time <w|b|e> <seconds|off>` - Set the time limit for an engine's turn. Use `off` to disable.
* `respect <w|b> <0.0-1.0>` - Set the time respect factor, adjusting an engine's clock management based on the opponent's remaining time.
* `unlimited` - Toggle whether time-limited engines are allowed to bypass their max depth constraint.
* `param <w|b|e>` - List all tunable parameters for the specified engine.
* `param <w|b|e> <id> <value>` - Set a specific engine parameter to a new float value.

### Tournament Reports
* `tstats` or `treport` - Print a detailed global tournament report, including final standings and termination breakdowns.
* `tstats <EngineA> / <EngineB>` - Print a detailed pairing drill-down between two specific engines.

### Help
* `help` or `h` - Show the complete command list in the terminal.

## Position Format
Use standard algebraic notation:
* **Files (columns)**: a, b, c, d, e, f, g, h (extends up to 'z' and beyond for larger variants).
* **Ranks (rows)**: 1, 2, 3, 4, 5, 6, 7, 8 (from White's perspective).
* **Examples**: `e2`, `d4`, `a1`, `h8`.

## Board Display Features

When using the `board` command, the console renders a visual representation of the game:
* ` ` - Empty square
* `P/p` - White/Black pieces (uppercase/lowercase)
* `[P]` - Source of the last move
* `<P>` - Destination of the last move
* Coordinate labels are printed on all sides.

Pieces are displayed using their first character from the piece configuration (e.g., `K/k` for King, `C/c` for Camel).

## Usage Examples

### Making Moves
```text
> move e2 e4
Attempting move: e2 -> e4
Move successful!
```

### Viewing the Board
```text
> board
         CURRENT BOARD
      a  b  c  d  e  f  g  h
   8  r  n  b  q  k  b  n  r  8
   7  p  p  p  p  p  p  p  p  7
   6                          6
   5                          5
   4             <P>          4
   3                          3
   2  P  P  P  P [ ] P  P  P  2
   1  R  N  B  Q  K  B  N  R  1
      a  b  c  d  e  f  g  h

Turn: Black
Last move: e2 -> e4 (shown as [piece] -> <piece>)
```

### Analysis
```text
> analyze
Analyzing position...

=== Comprehensive Position Analysis ===
MATERIAL ANALYSIS:
  White total: 39.20
  Black total: 39.20
  Material difference: 0.00 (+ = White advantage)
```

## User Interface Integration

### Terminal Input Field
* Located at the bottom of the GUI control panel.
* Type commands and press Enter to execute.
* Input field clears after each command.
* All commands provide rich console feedback.

### Dual Interface
* **GUI buttons**: Click to perform actions.
* **Terminal commands**: Type to perform the exact same actions.
* **Both work simultaneously**: Use whichever is more convenient at the moment.
