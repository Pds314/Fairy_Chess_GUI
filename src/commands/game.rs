use crate::app::ChessGui;
use crate::clog;
use crate::core::chain::BoardEvent;
use crate::core::game_state::MoveAttemptResult;
use crate::core::piece::Piece;
use crate::core::{DrawReason, GameMove, GameResult, Ghost, PieceColor, Position};
use std::path::Path;
use std::time::Instant;

impl ChessGui {
    pub(crate) fn cmd_board(&self) {
        let (rows, cols) = self.game_state.board.size();
        clog!("\n┌─────────────────────────────────┐");
        clog!("│         CURRENT BOARD         │");
        clog!("├─────────────────────────────────┤");

        let mut header = String::from("│   ");
        for col in 0..cols { header.push_str(&format!(" {} ", (b'a' + col as u8) as char)); }
        header.push_str(" │");
        clog!("{}", header);
        clog!("├─────────────────────────────────┤");

        for row in 0..rows {
            let mut line = format!("│ {} │", rows - row);
            for col in 0..cols {
                let pos = (row, col);
                let sym = match self.game_state.board.get_piece(pos) {
                    Some(p) => self.get_piece_symbol(&p),
                    None => '·',
                };
                let cell = if self.game_state.board.ghost_at(pos).is_some() {
                    format!("({})", sym)
                } else if let Some((from, to)) = self.last_move_highlight {
                    if pos == from { format!("[{}]", sym) }
                    else if pos == to { format!("<{}>", sym) }
                    else { format!(" {} ", sym) }
                } else {
                    format!(" {} ", sym)
                };
                line.push_str(&cell);
            }
            line.push_str(&format!("│ {}", rows - row));
            clog!("{}", line);
        }

        clog!("├─────────────────────────────────┤");
        let mut footer = String::from("│   ");
        for col in 0..cols { footer.push_str(&format!(" {} ", (b'a' + col as u8) as char)); }
        footer.push_str(" │");
        clog!("{}", footer);
        clog!("└─────────────────────────────────┘");

        clog!("Turn: {:?}", self.game_state.current_turn);
        clog!("Legend: [x] moved-from   <x> moved-to   (x) ghost square");
        if let Some((from, to)) = self.last_move_highlight {
            clog!("Last move: {} → {}", self.position_to_algebraic(from), self.position_to_algebraic(to));
        }
        let live = self.game_state.live_ghosts().len();
        if live > 0 { clog!("Live ghosts: {}  (type 'ghosts' for detail)", live); }
    }

    pub(crate) fn cmd_ghosts(&self) {
        clog!("\n👻 GHOSTS");
        clog!("{}", self.game_state.format_ghosts(&self.piece_config));
    }

    pub(crate) fn cmd_tape(&self, parts: &[&str]) {
        let last_n = match parts.get(1) {
            None => Some(3usize),
            Some(&"all") => None,
            Some(s) => match s.parse::<usize>() {
                Ok(v) => Some(v),
                Err(_) => { clog!("❌ Usage: tape [n | all]"); return; }
            },
        };
        clog!("\n🎞️  BOARD-OPERATION TAPE");
        clog!("{}", self.game_state.format_tape(&self.piece_config, last_n));
    }

    /// `perft <depth>` / `perft divide <d>` / `perft verify <d>` / `perft stats <d>`
    ///
    /// Perft is both the speed benchmark and — via `verify` — the proof that
    /// make/unmake is a bijection over hash, tape, ghost epoch, royal lists and
    /// piece counts.
    pub(crate) fn cmd_perft(&mut self, parts: &[&str]) {
        let (mode, depth) = match (parts.get(1), parts.get(2)) {
            (Some(&"divide"), Some(d)) => ("divide", d.parse::<u32>().ok()),
            (Some(&"verify"), Some(d)) => ("verify", d.parse::<u32>().ok()),
            (Some(&"stats"), Some(d)) => ("stats", d.parse::<u32>().ok()),
            (Some(d), None) => ("plain", d.parse::<u32>().ok()),
            _ => { clog!("❌ Usage: perft <depth> | perft divide|verify|stats <depth>"); return; }
        };
        let Some(depth) = depth else { clog!("❌ Depth must be a positive integer"); return; };
        if depth == 0 || depth > 12 { clog!("❌ Depth must be 1..=12"); return; }

        let mg = self.move_generator.clone();
        let cm = self.piece_config.clone();
        let t = Instant::now();

        match mode {
            "divide" => {
                let (rows, total) = self.game_state.perft_divide(&mg, &cm, depth);
                for (label, n) in &rows { clog!("  {:<8} {:>12}", label, n); }
                let e = t.elapsed();
                clog!("\nperft({}) = {}  in {:.3?}  ({:.0} nps, {} root moves)",
                      depth, total, e, total as f64 / e.as_secs_f64().max(1e-9), rows.len());
            }
            "verify" => {
                match self.game_state.perft_verify(&mg, &cm, depth) {
                    Ok(n) => {
                        let e = t.elapsed();
                        clog!("✅ perft_verify({}) = {} in {:.3?}", depth, n, e);
                        clog!("   Every node round-tripped: Zobrist hash, tape length, ghost stack");
                        clog!("   length and epoch, move history, fifty-move counter, piece counts,");
                        clog!("   royal and royalty position lists. make/unmake is a bijection.");
                    }
                    Err(e) => clog!("❌ INVARIANT VIOLATION\n{}", e),
                }
            }
            "stats" => {
                let s = self.game_state.perft_detailed(&mg, &cm, depth);
                let e = t.elapsed();
                clog!("perft({}) = {}   in {:.3?}  ({:.0} nps)", depth, s.nodes, e,
                      s.nodes as f64 / e.as_secs_f64().max(1e-9));
                clog!("  captures        {}", s.captures);
                clog!("  en passant      {}", s.en_passant);
                clog!("  castles         {}", s.castles);
                clog!("  promotions      {}", s.promotions);
                clog!("  flight captures {}", s.flight_captures);
            }
            _ => {
                let n = self.game_state.perft(&mg, &cm, depth);
                let e = t.elapsed();
                clog!("perft({}) = {}   in {:.3?}  ({:.0} nps)", depth, n, e,
                      n as f64 / e.as_secs_f64().max(1e-9));
            }
        }
    }

    /// `bench [perft_depth]` — micro-benchmarks plus the sizes of everything
    /// we move around per node.
    pub(crate) fn cmd_bench(&mut self, parts: &[&str]) {
        let depth = parts.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(4).clamp(1, 7);
        let mg = self.move_generator.clone();
        let cm = self.piece_config.clone();

        clog!("\n⏱️  BENCHMARK  (position: {} pieces, depth {})",
              self.game_state.board.count_pieces(), depth);
        clog!("Warming up…");
        let r = self.game_state.bench(&mg, &cm, depth);

        clog!("\n── Throughput ──");
        clog!("  pseudo-legal generations   {:>12.0} /s", r.pseudo_legal_per_sec);
        clog!("  legal generations          {:>12.0} /s", r.legal_per_sec);
        clog!("  make + unmake pairs        {:>12.0} /s", r.make_unmake_per_sec);
        clog!("  mover_king_in_check        {:>12.0} /s", r.check_tests_per_sec);
        clog!("  perft({})                   {:>12.0} nps   ({} nodes in {:.3?})",
              r.perft_depth, r.nps(), r.perft_nodes, r.perft_time);

        clog!("\n── What we move per node ──");
        clog!("  GameMove        {:>3} B   (pushed/popped once per make/unmake; Copy)",
              std::mem::size_of::<GameMove>());
        clog!("  BoardEvent      {:>3} B   (2-4 per move, contiguous on the tape)",
              std::mem::size_of::<BoardEvent>());
        clog!("  Piece           {:>3} B   ({} of them inside each BoardEvent)",
              std::mem::size_of::<Piece>(), 1);
        clog!("  Ghost           {:>3} B", std::mem::size_of::<Ghost>());
        clog!("  ExpandedMove    {:>3} B   (~30 per node, sorted by order_moves)",
              std::mem::size_of::<crate::core::ExpandedMove>());
        clog!("  Position        {:>3} B   (two usizes — the largest remaining tax)",
              std::mem::size_of::<Position>());

        clog!("\n── Allocations per pseudo-legal generation ──");
        clog!("  1 × Vec<ExpandedMove>  +  1 × reusable Vec<MoveWithPath> scratch");
        clog!("  (was: 1 × get_pieces_by_color + 1 per piece = ~17)");
        clog!("  Remaining per node: 2 × HashMap op on position_history.");
    }

    pub(crate) fn cmd_move(&mut self, parts: &[&str]) {
        if parts.len() >= 3 {
            if let (Some(from), Some(to)) = (self.parse_position(parts[1]), self.parse_position(parts[2])) {
                self.attempt_terminal_move(from, to);
            } else {
                clog!("❌ Invalid position format. Use algebraic notation (e.g., 'move e2 e4')");
            }
        } else {
            clog!("❌ Usage: move <from> <to> (e.g., 'move e2 e4')");
        }
    }

    pub(crate) fn cmd_undo(&mut self) { clog!("🔄 Undoing last move..."); self.handle_undo(); }

    pub(crate) fn cmd_redo(&mut self) {
        if self.game_state.redo_move(&self.move_generator, &self.piece_config) {
            clog!("✅ Move redone!");
            if let Some(last) = self.game_state.move_history.last() {
                self.last_move_highlight = Some((last.from, last.to));
            }
            self.cmd_board();
            self.check_for_engine_move();
        } else {
            clog!("❌ Nothing to redo");
        }
    }

    pub(crate) fn cmd_reset(&mut self) { clog!("🔄 Resetting board..."); self.handle_reset(); }

    pub(crate) fn cmd_loadpgn(&mut self, parts: &[&str]) {
        if parts.len() < 2 { clog!("❌ Usage: loadpgn <pgn_text_in_quotes_or_file_path>"); return; }
        let pgn_text = parts[1..].join(" ");
        let pgn_text = pgn_text.trim_matches('"');

        let pgn_content = if std::path::Path::new(pgn_text).exists() {
            match std::fs::read_to_string(pgn_text) {
                Ok(c) => c,
                Err(e) => { clog!("❌ Failed to read file: {}", e); return; }
            }
        } else { pgn_text.to_string() };

        self.handle_reset();
        match crate::pgn::PgnImporter::load_pgn(&pgn_content, &mut self.game_state, &self.move_generator, &self.piece_config) {
            Ok(count) => {
                clog!("✅ Loaded {} moves from PGN", count);
                if let Some(last) = self.game_state.move_history.last() {
                    self.last_move_highlight = Some((last.from, last.to));
                }
                self.cmd_board();
            }
            Err(e) => { clog!("❌ PGN loading failed: {}", e); self.cmd_board(); }
        }
    }

    pub(crate) fn cmd_status(&self) {
        clog!("🎮 GAME STATUS:");
        clog!("  Current turn: {:?}", self.game_state.current_turn);
        clog!("  Fifty-move counter: {}", self.game_state.fifty_move_counter);
        clog!("  Tape: {} events   Ghosts: {} live", self.game_state.tape_len(), self.game_state.live_ghosts().len());

        match &self.game_state.game_result {
            Some(GameResult::Winner(color)) => clog!("  Result: {:?} Wins", color),
            Some(GameResult::Draw(reason)) => clog!("  Result: Draw by {}", match reason {
                DrawReason::FiftyMoveRule => "fifty-move rule",
                DrawReason::Repetition => "repetition",
                DrawReason::Stalemate => "stalemate",
                DrawReason::InsufficientMaterial => "insufficient material",
                DrawReason::MutualElimination => "mutual royal elimination",
            }),
            Some(GameResult::Ongoing) => clog!("  Result: Game in progress"),
            None => clog!("  Result: Unknown"),
        }
        if let Some(ref file) = self.current_game_file {
            clog!("  Game file: {}", Path::new(file).file_name().unwrap_or_default().to_string_lossy());
        }
    }

    pub(crate) fn attempt_terminal_move(&mut self, from: Position, to: Position) {
        let from_alg = self.position_to_algebraic(from);

        if let Some(piece) = self.game_state.board.get_piece(from) {
            if piece.color != self.game_state.current_turn {
                clog!("❌ Not your piece! It's {:?}'s turn.", self.game_state.current_turn);
                return;
            }
            clog!("🎯 Attempting move: {} → {}", from_alg, self.position_to_algebraic(to));

            match self.game_state.attempt_move(from, to, &self.move_generator, &self.piece_config) {
                MoveAttemptResult::Success => {
                    self.game_state.clear_redo();
                    self.last_move_highlight = Some((from, to));
                    self.position_analysis = None;
                    self.game_controller.end_turn(self.game_state.current_turn.opposite());
                    clog!("✅ Move successful!");

                    let size = self.game_state.board.size();
                    for ev in self.game_state.last_chain() {
                        clog!("   {}", ev.describe(size, &self.piece_config));
                    }
                    let n = self.game_state.move_count();
                    for g in self.game_state.ghosts_of(n - 1) {
                        clog!("   ghost {} -> {}  [{}]{}",
                              crate::notation::position_to_algebraic(g.square(), size),
                              crate::notation::position_to_algebraic(g.owner(), size),
                              g.flags().dsl_marks(),
                              if self.game_state.ghost_projects_royalty(g) { "  (projects royalty)" } else { "" });
                    }

                    self.cmd_board();
                    self.check_for_engine_move();
                }
                MoveAttemptResult::Invalid => clog!("❌ Invalid move!"),
                MoveAttemptResult::NeedsCastlingChoice => clog!("🏰 Castling move detected - use GUI for castling selection."),
                MoveAttemptResult::NeedsPromotion => clog!("👑 Promotion required - use GUI for piece selection."),
            }
        } else {
            clog!("❌ No piece at {}", from_alg);
        }
    }

    pub(crate) fn parse_position(&self, pos_str: &str) -> Option<Position> {
        if pos_str.len() < 2 { return None; }
        let chars: Vec<char> = pos_str.chars().collect();
        let col_char = chars[0].to_ascii_lowercase();
        if let Ok(rank) = pos_str[1..].parse::<usize>() {
            let col = (col_char as u8).wrapping_sub(b'a') as usize;
            let (rows, cols) = self.game_state.board.size();
            if rank > 0 && rank <= rows && col < cols { return Some((rows - rank, col)); }
        }
        None
    }

    pub(crate) fn position_to_algebraic(&self, pos: Position) -> String {
        let (rows, _) = self.game_state.board.size();
        format!("{}{}", (b'a' + pos.1 as u8) as char, rows - pos.0)
    }

    pub(crate) fn get_piece_symbol(&self, piece: &Piece) -> char {
        if let Some(pc) = self.piece_config.get_piece_by_index(piece.piece_type) {
            if let Some(symbol) = pc.characters.first() {
                let base = symbol.chars().next().unwrap_or('?');
                return match piece.color {
                    PieceColor::White => base.to_ascii_uppercase(),
                    PieceColor::Black => base.to_ascii_lowercase(),
                };
            }
        }
        '?'
    }
}
