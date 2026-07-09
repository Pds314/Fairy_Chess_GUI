// src/core/game_state.rs
//! Core GameState: definition, construction, hashing, basic queries, and the
//! diagnostic surface for ghosts and the board-operation tape.

use crate::attack_table::{AttackStats, AttackTable, CoverageRays};
use crate::board_config::BoardConfig;
use crate::core::board::Board;
use crate::core::chain::BoardEvent;
use crate::core::game_types::*;
use crate::core::ghost::Ghost;
use crate::core::piece::PieceColor;
use crate::insufficient_material::InsufficientMaterialRules;
use crate::move_generator::MoveGenerator;
use crate::notation::position_to_algebraic;
use crate::piece_config::PieceConfigManager;
use crate::promotion::PromotionConfig;
use crate::zobrist::turn_key;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

pub use super::game_types::{
    DrawReason, ExpandedMove, GameMove, GameResult, MateStatus, MoveAttemptResult,
    MoveGenerationResult, PendingMove, PerformanceSnapshot, PerformanceTracker,
};

#[derive(Debug)]
pub struct GameState {
    pub board: Board,
    pub current_turn: PieceColor,
    pub move_history: Vec<GameMove>,
    pub redo_stack: Vec<GameMove>,
    pub fifty_move_counter: u32,
    pub position_history: HashMap<u64, u32>,
    pub game_result: Option<GameResult>,
    pub pending_move: Option<PendingMove>,
    pub promotion_config: PromotionConfig,
    pub insufficient_material: InsufficientMaterialRules,
    pub performance: PerformanceTracker,
    pub uses_royal_system: bool,
    pub(crate) tape: Vec<BoardEvent>,
    pub(crate) attack_table: Option<Box<AttackTable>>,
}

impl Clone for GameState {
    fn clone(&self) -> Self {
        Self {
            board: self.board.clone(),
            current_turn: self.current_turn,
            move_history: self.move_history.clone(),
            redo_stack: Vec::new(),
            fifty_move_counter: self.fifty_move_counter,
            position_history: self.position_history.clone(),
            game_result: self.game_result.clone(),
            pending_move: self.pending_move.clone(),
            promotion_config: self.promotion_config.clone(),
            insufficient_material: self.insufficient_material.clone(),
            performance: PerformanceTracker::new(),
            uses_royal_system: self.uses_royal_system,
            tape: self.tape.clone(),
            attack_table: self.attack_table.clone(),
        }
    }
}

impl GameState {
    pub fn new(config_manager: &PieceConfigManager, move_generator: &mut MoveGenerator) -> Self {
        let board = Board::new(config_manager);
        Self::init_with_board(board, PieceColor::White, 0, PromotionConfig::default(),
                              InsufficientMaterialRules::empty(), config_manager, move_generator)
    }

    pub fn from_config(
        config: BoardConfig,
        config_manager: &PieceConfigManager,
        move_generator: &mut MoveGenerator,
    ) -> Self {
        let board = config.create_board(config_manager);
        let promotion_config = config.promotion_config.clone();
        let insufficient_material =
        InsufficientMaterialRules::compile(&config.insufficient_material, config_manager);
        move_generator.set_zones(config.zones.clone());
        Self::init_with_board(board, config.starting_player, config.fifty_move_counter,
                              promotion_config, insufficient_material, config_manager, move_generator)
    }

    pub(crate) fn init_with_board(
        board: Board,
        starting_turn: PieceColor,
        fifty_move_counter: u32,
        promotion_config: PromotionConfig,
        insufficient_material: InsufficientMaterialRules,
        _config_manager: &PieceConfigManager,
        move_generator: &mut MoveGenerator,
    ) -> Self {
        move_generator.precompute_moves_for_board(board.size());

        let uses_royal_system = !board.get_royal_positions(PieceColor::White).is_empty()
        || !board.get_royal_positions(PieceColor::Black).is_empty()
        || board.royalty_count(PieceColor::White) > 0
        || board.royalty_count(PieceColor::Black) > 0;

        let initial_hash = board.piece_hash() ^ turn_key(starting_turn);
        let mut position_history = HashMap::new();
        position_history.insert(initial_hash, 1);

        let mut state = Self {
            board,
            current_turn: starting_turn,
            move_history: Vec::with_capacity(128),
            redo_stack: Vec::new(),
            fifty_move_counter,
            position_history,
            game_result: Some(GameResult::Ongoing),
            pending_move: None,
            promotion_config,
            insufficient_material,
            performance: PerformanceTracker::new(),
            uses_royal_system,
            tape: Vec::with_capacity(512),
            attack_table: None,
        };
        state.check_draw_conditions();
        state
    }

    #[inline(always)]
    pub fn current_hash(&self) -> u64 {
        self.board.piece_hash() ^ turn_key(self.current_turn)
    }
    #[inline]
    pub fn calculate_current_hash(&self, _cm: &PieceConfigManager) -> u64 {
        self.current_hash()
    }

    pub fn is_valid_turn(&self, from: crate::core::position::Position) -> bool {
        self.board.get_piece(from).map_or(false, |p| p.color == self.current_turn)
    }
    pub fn is_ongoing(&self) -> bool {
        matches!(self.game_result, Some(GameResult::Ongoing))
    }
    pub fn can_undo(&self) -> bool { !self.move_history.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo_stack.is_empty() }
    pub fn clear_redo(&mut self) { self.redo_stack.clear(); }
    pub fn move_count(&self) -> usize { self.move_history.len() }
    pub fn get_position_string(&self, cm: &PieceConfigManager) -> String {
        self.board.to_position_string(cm)
    }

    // ─── Ghost diagnostics ──────────────────────────────────────────

    #[inline]
    pub fn live_ghosts(&self) -> &[Ghost] {
        self.board.live_ghosts()
    }

    /// Royalty projection is derived, never declared.
    pub fn ghost_projects_royalty(&self, g: &Ghost) -> bool {
        match self.board.get_piece(g.owner()) {
            Some(o) => o.is_royal || (o.is_royalty && self.board.royalty_count(o.color) == 1),
            None => false,
        }
    }

    /// Ghosts pushed by `move_history[i]`. The stack is append-only, and each
    /// frame records the previous epoch start, so this reconstructs losslessly
    /// — itself the proof that no `born_ply` counter is needed.
    pub fn ghosts_of(&self, i: usize) -> &[Ghost] {
        let n = self.move_history.len();
        if i >= n { return &[]; }
        let start = if i + 1 < n {
            self.move_history[i + 1].ghost_live_start as usize
        } else {
            self.board.ghost_epoch() as usize
        };
        let end = if i + 2 < n {
            self.move_history[i + 2].ghost_live_start as usize
        } else if i + 1 < n {
            self.board.ghost_epoch() as usize
        } else {
            self.board.ghost_len()
        };
        let all = self.board.all_ghosts();
        if start <= end && end <= all.len() { &all[start..end] } else { &[] }
    }

    pub fn format_ghosts(&self, cm: &PieceConfigManager) -> String {
        let size = self.board.size();
        let mut s = String::new();
        let live = self.live_ghosts();

        let _ = writeln!(s, "Ghost stack: {} total, {} live (epoch starts at {})",
                         self.board.ghost_len(), live.len(), self.board.ghost_epoch());
        if live.is_empty() {
            let _ = writeln!(s, "  (no live ghosts)");
            return s;
        }
        let _ = writeln!(s, "  {:<6} {:<6} {:<18} {:<8} {:<22} {}",
                         "square", "owner", "owner piece", "marks", "flags", "projects royalty?");
        for g in live {
            let owner_desc = match self.board.get_piece(g.owner()) {
                Some(p) => {
                    let name = cm.get_piece_by_index(p.piece_type)
                    .map(|c| c.display_name.clone()).unwrap_or_else(|| "?".into());
                    format!("{} {} {:?}", p.to_char(cm), name, p.color)
                }
                None => "(owner gone)".to_string(),
            };
            let _ = writeln!(s, "  {:<6} {:<6} {:<18} {:<8} {:<22} {}",
                             position_to_algebraic(g.square(), size),
                             position_to_algebraic(g.owner(), size),
                             owner_desc, g.flags().dsl_marks(), g.flags().to_string(),
                             if self.ghost_projects_royalty(g) { "YES" } else { "no" });
        }
        s
    }

    // ─── Board-operation tape diagnostics ───────────────────────────

    #[inline]
    pub fn tape(&self) -> &[BoardEvent] { &self.tape }
    #[inline]
    pub fn tape_len(&self) -> usize { self.tape.len() }

    pub fn events_of(&self, i: usize) -> &[BoardEvent] {
        let n = self.move_history.len();
        if i >= n { return &[]; }
        let start = self.move_history[i].tape_start as usize;
        let end = if i + 1 < n { self.move_history[i + 1].tape_start as usize } else { self.tape.len() };
        &self.tape[start..end]
    }

    pub fn last_chain(&self) -> &[BoardEvent] {
        if self.move_history.is_empty() { &[] } else { self.events_of(self.move_history.len() - 1) }
    }

    pub fn mover_of(&self, i: usize) -> PieceColor {
        let n = self.move_history.len();
        if (n - 1 - i) % 2 == 0 { self.current_turn.opposite() } else { self.current_turn }
    }

    pub fn format_tape(&self, cm: &PieceConfigManager, last_n: Option<usize>) -> String {
        let size = self.board.size();
        let n = self.move_history.len();
        let start = last_n.map(|k| n.saturating_sub(k)).unwrap_or(0);

        let mut s = String::new();
        let _ = writeln!(s, "Tape: {} events over {} moves   |   Ghost stack: {} ({} live)   |   GameMove = {} B",
                         self.tape.len(), n, self.board.ghost_len(), self.live_ghosts().len(),
                         std::mem::size_of::<GameMove>());
        if n == 0 {
            let _ = writeln!(s, "  (no moves yet)");
            return s;
        }
        if start > 0 {
            let _ = writeln!(s, "  … {} earlier move(s) elided", start);
        }

        for i in start..n {
            let gm = self.move_history[i];
            let _ = writeln!(s, "\n#{:<4} {:<5} {} -> {}   tape[{}..{}]   fifty {}   hash 0x{:016x}",
                             i + 1, format!("{:?}", self.mover_of(i)),
                             position_to_algebraic(gm.from, size), position_to_algebraic(gm.to, size),
                             gm.tape_start, gm.tape_start as usize + self.events_of(i).len(),
                             gm.fifty_move_counter_before_move, gm.piece_hash_before);

            for ev in self.events_of(i) {
                let _ = writeln!(s, "      {}", ev.describe(size, cm));
            }
            for g in self.ghosts_of(i) {
                let _ = writeln!(s, "     ghost {} -> {}  [{}]  {}{}",
                                 position_to_algebraic(g.square(), size),
                                 position_to_algebraic(g.owner(), size),
                                 g.flags().dsl_marks(), g.flags(),
                                 if self.ghost_projects_royalty(g) { "  (projects royalty)" } else { "" });
            }
            if let Some((rf, rt)) = gm.castling_rook_move() {
                let _ = writeln!(s, "      [castling partner {} -> {}]",
                                 position_to_algebraic(rf, size), position_to_algebraic(rt, size));
            }
            if gm.is_en_passant_capture() {
                let _ = writeln!(s, "      [capture via ghost alias]");
            }
            if let (Some(f), Some(t)) = (gm.promoted_from(), gm.promoted_to()) {
                let name = |x: usize| cm.get_piece_by_index(x).map(|c| c.display_name.clone()).unwrap_or_else(|| "?".into());
                let _ = writeln!(s, "      [promotion {} -> {}]", name(f), name(t));
            }
            if gm.flight_capture_count() > 0 {
                let _ = writeln!(s, "      [{} flight capture(s)]", gm.flight_capture_count());
            }
        }
        s
    }

    // ─── Performance ────────────────────────────────────────────────

    pub fn get_performance_stats(&self) -> PerformanceSnapshot { self.performance.snapshot() }
    pub fn reset_performance_stats(&self) { self.performance.reset(); }
    pub fn clone_for_worker(&self) -> Self { self.clone() }

    // ─── Optional incremental attack table ──────────────────────────

    pub fn enable_attack_table(&mut self, rays: Arc<CoverageRays>) {
        self.attack_table = Some(Box::new(AttackTable::new(rays, &self.board)));
    }
    pub fn disable_attack_table(&mut self) { self.attack_table = None; }
    #[inline]
    pub fn has_attack_table(&self) -> bool { self.attack_table.is_some() }
    #[inline]
    pub fn sync_attack_table(&mut self) {
        if let Some(t) = self.attack_table.as_mut() { t.sync(&self.board); }
    }
    #[inline]
    pub fn attacks(&self) -> Option<&AttackTable> { self.attack_table.as_deref() }
    pub fn attack_stats(&self) -> Option<AttackStats> { self.attack_table.as_ref().map(|t| t.stats()) }
    pub fn reset_attack_stats(&mut self) {
        if let Some(t) = self.attack_table.as_mut() { t.reset_stats(); }
    }
}
