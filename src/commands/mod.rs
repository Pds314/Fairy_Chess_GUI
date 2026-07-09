pub(crate) mod analysis;
pub(crate) mod engine;
pub(crate) mod evolution;
pub(crate) mod game;

use crate::app::ChessGui;
use crate::clog;
use std::io::IsTerminal;

pub(crate) fn collect_startup_commands() -> Vec<String> {
    let mut commands = Vec::new();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if (args[i] == "-c" || args[i] == "--command") && i + 1 < args.len() {
            commands.push(args[i + 1].clone());
            i += 2;
        } else if (args[i] == "-f" || args[i] == "--file") && i + 1 < args.len() {
            if let Ok(contents) = std::fs::read_to_string(&args[i + 1]) {
                for line in contents.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        commands.push(trimmed.to_string());
                    }
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    if !std::io::stdin().is_terminal() {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines().flatten() {
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                commands.push(trimmed);
            }
        }
    }
    commands
}

impl ChessGui {
    pub(crate) fn execute_startup_commands(&mut self) {
        let commands = std::mem::take(&mut self.startup_commands);
        for command in commands {
            clog!("📝 Executing startup command: {}", command);
            self.handle_terminal_command(&command);
        }
    }

    pub(crate) fn handle_terminal_command(&mut self, command: &str) {
        let command = command.trim();
        clog!("\n> {}", command);
        if command.is_empty() { return; }

        let parts: Vec<&str> = command.split_whitespace().collect();
        let cmd = parts[0].to_lowercase();

        match cmd.as_str() {
            "help" | "h" => self.print_terminal_help(),
            "board" | "b" => self.cmd_board(),
            "move" | "m" => self.cmd_move(&parts),
            "undo" | "u" => self.cmd_undo(),
            "redo" => self.cmd_redo(),
            "loadpgn" => self.cmd_loadpgn(&parts),
            "reset" | "r" => self.cmd_reset(),

            // ── Move-system diagnostics & measurement ──────────────
            "ghosts" | "gh" => self.cmd_ghosts(),
            "tape" | "ops" | "events" => self.cmd_tape(&parts),
            "perft" => self.cmd_perft(&parts),
            "bench" => self.cmd_bench(&parts),

            "analyze" | "a" => self.cmd_analyze(),
            "moves" => { clog!("📋 Generating all legal moves..."); self.handle_generate_moves(); }
            "eval" => { clog!("⚖️ Evaluating position..."); self.handle_evaluate_position(); }
            "best" => self.cmd_best(),
            "engine" => self.cmd_engine(&parts),
            "turn" => clog!("Current turn: {:?}", self.game_state.current_turn),
            "depth" => self.cmd_depth(&parts),
            "time" => self.cmd_time(&parts),
            "respect" => self.cmd_respect(&parts),
            "unlimited" => self.cmd_unlimited(),
            "status" => self.cmd_status(),
            "tstats" | "treport" => self.cmd_tstats(&parts),
            "pgn" | "p" => { clog!("📄 Printing game PGN..."); self.print_pgn(); }
            "param" | "parameter" => self.cmd_param(&parts),
            "evolve" => self.cmd_evolve(&parts),
            _ => clog!("❌ Unknown command: '{}'. Type 'help' for available commands.", cmd),
        }
    }

    fn print_terminal_help(&self) {
        clog!("📚 TERMINAL COMMANDS:");
        clog!("  help, h                    - Show this help");
        clog!("  board, b                   - Pretty print the board ((x) = ghost square)");
        clog!("  move <from> <to>           - Make a move (prints its atomic board ops)");
        clog!("  undo, u / redo / reset, r  - History");
        clog!("");
        clog!("  ── Move system: diagnostics & measurement ──");
        clog!("  ghosts, gh                 - Live ghosts: square → owner, flags, projection");
        clog!("  tape [n|all], ops, events  - Board-operation history (default: last 3 moves)");
        clog!("  perft <d>                  - Leaf-node count + nodes/sec");
        clog!("  perft divide <d>           - Per-root-move breakdown (bisect a mismatch)");
        clog!("  perft stats <d>            - Captures / e.p. / castles / promotions / flight");
        clog!("  perft verify <d>           - Prove make/unmake is a bijection at every node");
        clog!("  bench [d]                  - Throughput + the size of everything we move");
        clog!("");
        clog!("  analyze, a                 - Run comprehensive position analysis");
        clog!("  moves / eval / best        - Move list, evaluation, engine move");
        clog!("  engine [type]              - Set engine or show engine status");
        clog!("  depth <w|b|e> <n>          - Set search depth");
        clog!("  time <w|b|e> <seconds>     - Set time limit (or 'off')");
        clog!("  respect <w|b> <0.0-1.0>    - Set time respect factor");
        clog!("  unlimited                  - Toggle unlimited depth with time limits");
        clog!("  turn / status              - Game state (incl. tape/ghost counts)");
        clog!("  tstats [<A> / <B>]         - Tournament report");
        clog!("  pgn, p / loadpgn <text>    - PGN export / import");
        clog!("  param <w|b|e> [id] [value] - Engine parameters");
        clog!("  evolve ...                 - Parameter evolution");
    }

    fn cmd_tstats(&mut self, parts: &[&str]) {
        if self.tournament.elo.game_log().is_empty() {
            clog!("No tournament data yet.");
        } else if parts.len() >= 3 {
            let joined = parts[1..].join(" ");
            let mut split = joined.splitn(2, '/');
            match (split.next(), split.next()) {
                (Some(a), Some(b)) => {
                    match (
                        crate::engine::personality::parse_engine_name(a.trim()),
                           crate::engine::personality::parse_engine_name(b.trim()),
                    ) {
                        (Some(ea), Some(eb)) => self.tournament.elo.print_pairing_detail(&ea, &eb),
                        _ => clog!("❌ Unknown engine name. Use `tstats` alone for the full report."),
                    }
                }
                _ => clog!("❌ Usage: tstats <engineA> / <engineB>"),
            }
        } else {
            self.tournament.elo.print_detailed_report();
        }
    }
}
