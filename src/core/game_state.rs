// src/core/game_state.rs
use crate::board_config::BoardConfig;
use crate::core::board::{Board, EnPassantTarget};
use crate::core::piece::{Piece, PieceColor};
use crate::core::position::Position;
use crate::insufficient_material::InsufficientMaterialRules;
use crate::move_generator::{CastlingOption, CompiledMove, MoveGenerator, MoveWithPath};
use crate::piece_config::PieceConfigManager;
use crate::promotion::{
    PromotionConfig, PromotionManager, PromotionSelector, RandomPromotionSelector,
};
use crate::zobrist::turn_key;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MateStatus {
    Ongoing,
    Checkmate,
    Stalemate,
    OpponentLostByCheck,
}

#[derive(Debug, Clone)]
pub struct ExpandedMove {
    pub from: Position,
    pub to: Position,
    pub move_with_path: MoveWithPath,
    pub castling_option: Option<CastlingOption>,
    pub promotion_target: Option<usize>,
    pub captures: Option<Piece>,
    pub captures_position: Option<Position>,
}

#[derive(Debug)]
pub enum MoveGenerationResult {
    Moves(Vec<ExpandedMove>),
    Checkmate {
        move_that_captures_royal: ExpandedMove,
    },
}

#[derive(Debug, Clone)]
pub struct GameMove {
    pub from: Position,
    pub to: Position,
    pub captured_piece: Option<Piece>,
    pub fifty_move_counter_before_move: u32,
    pub en_passant_targets_before: Vec<EnPassantTarget>,
    pub captured_en_passant: Option<Position>,
    pub captured_en_passant_piece: Option<Piece>,
    pub castling_rook_move: Option<(Position, Position)>,
    pub castling_rook_capture: Option<Piece>,
    pub castling_rights_before: CastlingRights,
    pub promoted_from: Option<usize>,
    pub promoted_to: Option<usize>,
    pub piece_hash_before: u64,
}

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
    pub castling_rights: CastlingRights,
    pub promotion_config: PromotionConfig,
    /// Dead-material draw table. Immutable for the life of the game;
    /// consulted once per move in `check_draw_conditions`. See the
    /// `insufficient_material` module for the cost profile.
    pub insufficient_material: InsufficientMaterialRules,
    pub performance: Arc<Mutex<PerformanceTracker>>,
}

#[derive(Debug, Clone)]
pub enum PendingMove {
    Castling {
        king_from: Position,
        king_to: Position,
        king_move: MoveWithPath,
        options: Vec<CastlingOption>,
    },
    Promotion {
        from: Position,
        to: Position,
        move_rule: MoveWithPath,
        targets: Vec<usize>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameResult {
    Winner(PieceColor),
    Draw(DrawReason),
    Ongoing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawReason {
    FiftyMoveRule,
    Repetition,
    Stalemate,
    InsufficientMaterial,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoveAttemptResult {
    Success,
    Invalid,
    NeedsCastlingChoice,
    NeedsPromotion,
}

#[derive(Debug, Clone, Default)]
pub struct PerformanceTracker {
    pub moves_made: usize,
    pub moves_undone: usize,
    pub moves_generated: usize,
    pub pseudo_legal_generations: usize,
    pub legal_move_checks: usize,
    pub check_tests: usize,
    pub mate_status_checks: usize,
}
impl PerformanceTracker {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn reset(&mut self) {
        *self = Default::default();
    }
}

#[derive(Debug, Clone)]
pub struct CastlingRights {
    pub pieces_with_rights: HashSet<Position>,
}
impl CastlingRights {
    pub fn new() -> Self {
        Self {
            pieces_with_rights: HashSet::new(),
        }
    }
    pub fn initialize(&mut self, board: &Board, move_generator: &MoveGenerator) {
        for row in 0..board.rows() {
            for col in 0..board.cols() {
                let pos = (row, col);
                if let Some(piece) = board.get_piece(pos) {
                    if move_generator.can_piece_castle(board, pos, piece.piece_type) {
                        self.pieces_with_rights.insert(pos);
                    }
                }
            }
        }
    }
    pub fn remove_rights(&mut self, pos: Position) {
        self.pieces_with_rights.remove(&pos);
    }
    pub fn update_position(&mut self, from: Position, to: Position) {
        if self.pieces_with_rights.remove(&from) {
            self.pieces_with_rights.insert(to);
        }
    }
    pub fn has_rights(&self, pos: Position) -> bool {
        self.pieces_with_rights.contains(&pos)
    }
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
            castling_rights: self.castling_rights.clone(),
            promotion_config: self.promotion_config.clone(),
            insufficient_material: self.insufficient_material.clone(),
            performance: Arc::clone(&self.performance),
        }
    }
}

// ─── Flag lookups ───────────────────────────────────────────────────────
// Both flags are needed at promotion sites, so we provide a combined
// helper. The single-flag helpers are kept for readability where only
// one is needed.
#[allow(dead_code)]
fn is_royal_type(piece_type: usize, cm: &PieceConfigManager) -> bool {
    cm.get_piece_by_index(piece_type)
        .map_or(false, |c| c.properties.is_royal)
}

#[allow(dead_code)]
fn is_royalty_type(piece_type: usize, cm: &PieceConfigManager) -> bool {
    cm.get_piece_by_index(piece_type)
        .map_or(false, |c| c.properties.is_royalty)
}

/// Look up both royal flags at once. Used at every promotion/demotion
/// site so the cached Piece flags stay consistent with the config.
fn piece_flags(piece_type: usize, cm: &PieceConfigManager) -> (bool, bool) {
    cm.get_piece_by_index(piece_type)
        .map_or((false, false), |c| {
            (c.properties.is_royal, c.properties.is_royalty)
        })
}

impl GameState {
    pub fn new(config_manager: &PieceConfigManager, move_generator: &mut MoveGenerator) -> Self {
        let board = Board::new(config_manager);
        Self::init_with_board(
            board,
            PieceColor::White,
            0,
            PromotionConfig::default(),
            InsufficientMaterialRules::empty(),
            config_manager,
            move_generator,
        )
    }

    pub fn from_config(
        config: BoardConfig,
        config_manager: &PieceConfigManager,
        move_generator: &mut MoveGenerator,
    ) -> Self {
        let board = config.create_board(config_manager);
        let promotion_config = config.promotion_config.clone();
        // Piece characters in the rules can only be resolved now that the
        // piece set is loaded — BoardConfig parsing didn't have it yet.
        let insufficient_material =
            InsufficientMaterialRules::compile(&config.insufficient_material, config_manager);
        move_generator.set_zones(config.zones.clone());

        Self::init_with_board(
            board,
            config.starting_player,
            config.fifty_move_counter,
            promotion_config,
            insufficient_material,
            config_manager,
            move_generator,
        )
    }

    fn init_with_board(
        board: Board,
        starting_turn: PieceColor,
        fifty_move_counter: u32,
        promotion_config: PromotionConfig,
        insufficient_material: InsufficientMaterialRules,
        _config_manager: &PieceConfigManager,
        move_generator: &mut MoveGenerator,
    ) -> Self {
        move_generator.precompute_moves_for_board(board.size());

        let initial_hash = board.piece_hash() ^ turn_key(starting_turn);

        let mut position_history = HashMap::new();
        position_history.insert(initial_hash, 1);
        let mut castling_rights = CastlingRights::new();
        castling_rights.initialize(&board, move_generator);

        let mut state = Self {
            board,
            current_turn: starting_turn,
            move_history: Vec::new(),
            redo_stack: Vec::new(),
            fifty_move_counter,
            position_history,
            game_result: Some(GameResult::Ongoing),
            pending_move: None,
            castling_rights,
            promotion_config,
            insufficient_material,
            performance: Arc::new(Mutex::new(PerformanceTracker::new())),
        };

        // Catch positions that are *already* drawn as loaded (dead
        // material, or a test FEN with fifty_move ≥ 100). Runs once per
        // game construction — not a hot path.
        state.check_draw_conditions();
        state
    }

    pub fn get_performance_stats(&self) -> PerformanceTracker {
        self.performance.lock().unwrap().clone()
    }
    pub fn reset_performance_stats(&self) {
        self.performance.lock().unwrap().reset();
    }

    pub fn is_in_check(
        &self,
        move_generator: &MoveGenerator,
        _config_manager: &PieceConfigManager,
    ) -> bool {
        self.performance.lock().unwrap().check_tests += 1;
        self.is_in_check_fast(move_generator)
    }

    /// Is the side to move currently in check?
    ///
    /// A side is in check if any of its 'R' pieces is attacked, OR — when
    /// it has exactly one 'r' piece remaining — that piece is attacked.
    /// The last 'r' piece dynamically inherits check protection because
    /// losing it would end the game; while multiple 'r' pieces remain,
    /// none individually carries that weight and they may be freely hung.
    #[inline]
    pub fn is_in_check_fast(&self, move_generator: &MoveGenerator) -> bool {
        let attacker = self.current_turn.opposite();

        // 'R' pieces: each one is always check-relevant.
        for &pos in self.board.get_royal_positions(self.current_turn) {
            if move_generator.is_square_attacked(&self.board, pos, attacker) {
                return true;
            }
        }

        // 'r' pieces: only the last one is check-relevant.
        let royalty = self.board.get_royalty_positions(self.current_turn);
        if royalty.len() == 1 {
            if move_generator.is_square_attacked(&self.board, royalty[0], attacker) {
                return true;
            }
        }
        false
    }

    /// Is the player who just moved now in check?
    ///
    /// A royal is in check if its own square is capture‑reachable, OR if any
    /// live en‑passant ghost that points at it is capture‑reachable. The second
    /// clause is what makes an `E`‑flagged move "unable to pass through check"
    /// without any castling‑specific code: after the move, the trail squares
    /// behave like the royal for one ply.
    #[inline]
    pub fn mover_king_in_check(&self, move_generator: &MoveGenerator) -> bool {
        let mover = self.current_turn.opposite();
        let attacker = self.current_turn;

        let royalty = self.board.get_royalty_positions(mover);
        let last_ry = royalty.len() == 1;

        let is_protected = |p: &Piece| p.is_royal || (p.is_royalty && last_ry);

        for &pos in self.board.get_royal_positions(mover) {
            if move_generator.is_square_attacked(&self.board, pos, attacker) {
                return true;
            }
        }
        if last_ry {
            if move_generator.is_square_attacked(&self.board, royalty[0], attacker) {
                return true;
            }
        }

        // Ghost squares of protected pieces. The target list is short (0–3 in
        // every shipped variant) and only contains entries from *this* move, so
        // the colour check is almost always true and the inner attack test runs
        // at most a couple of times — replacing, not adding to, the work the
        // old hardcoded castling path‑walk did.
        for ep in self.board.get_en_passant_targets() {
            if let Some(p) = self.board.get_piece(ep.piece_position) {
                if p.color == mover && is_protected(&p) {
                    if move_generator.is_square_attacked(&self.board, ep.position, attacker) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Mirror of the ghost logic for the "did the opponent leave themselves
    /// capturable" question. (generate_pseudo_legal_moves already catches the
    /// concrete capture via is_fatal_capture; this keeps the standalone helper
    /// consistent for any engine that calls it directly.)
    pub fn opponent_can_capture_royal(
        &self,
        move_generator: &MoveGenerator,
        _cm: &PieceConfigManager,
    ) -> bool {
        let victim = self.current_turn.opposite();
        let attacker = self.current_turn;

        let royalty = self.board.get_royalty_positions(victim);
        let last_ry = royalty.len() == 1;

        for &pos in self.board.get_royal_positions(victim) {
            if move_generator.is_square_attacked(&self.board, pos, attacker) {
                return true;
            }
        }
        if last_ry {
            if move_generator.is_square_attacked(&self.board, royalty[0], attacker) {
                return true;
            }
        }
        for ep in self.board.get_en_passant_targets() {
            if let Some(p) = self.board.get_piece(ep.piece_position) {
                if p.color == victim && (p.is_royal || (p.is_royalty && last_ry)) {
                    if move_generator.is_square_attacked(&self.board, ep.position, attacker) {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn opponent_in_check(
        &self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        self.performance.lock().unwrap().check_tests += 1;
        matches!(
            self.generate_pseudo_legal_moves(move_generator, config_manager),
            MoveGenerationResult::Checkmate { .. }
        )
    }

    pub fn has_legal_moves(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        self.performance.lock().unwrap().legal_move_checks += 1;
        !self
            .get_legal_moves(move_generator, config_manager)
            .is_empty()
    }

    /// Classify the position. Adds extinction detection: if the side to
    /// move has no legal moves AND no pieces at all, that is a loss (not
    /// a stalemate). This only fires in variants with no royal pieces —
    /// if any 'R' or last-'r' piece exists, it can't be captured without
    /// already being checkmate, so piece_count never reaches zero.
    pub fn get_mate_status(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> MateStatus {
        self.performance.lock().unwrap().mate_status_checks += 1;
        if matches!(
            self.generate_pseudo_legal_moves(move_generator, config_manager),
            MoveGenerationResult::Checkmate { .. }
        ) {
            return MateStatus::OpponentLostByCheck;
        }
        if self
            .get_legal_moves(move_generator, config_manager)
            .is_empty()
        {
            if self.is_in_check(move_generator, config_manager) {
                MateStatus::Checkmate
            } else if !self.board.has_pieces(self.current_turn) {
                // Extinction: no pieces left. This is a loss for the
                // side to move, which we report as Checkmate (same
                // downstream effect: current player loses).
                MateStatus::Checkmate
            } else {
                MateStatus::Stalemate
            }
        } else {
            MateStatus::Ongoing
        }
    }

    pub fn update_mate_status(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        if !matches!(self.game_result, Some(GameResult::Ongoing)) {
            return;
        }

        match self.get_mate_status(move_generator, config_manager) {
            MateStatus::Checkmate => {
                self.game_result = Some(GameResult::Winner(self.current_turn.opposite()));
            }
            MateStatus::Stalemate => {
                self.game_result = Some(GameResult::Draw(DrawReason::Stalemate));
            }
            MateStatus::OpponentLostByCheck => {
                self.game_result = Some(GameResult::Winner(self.current_turn));
            }
            MateStatus::Ongoing => {}
        }
    }

    // All castling‑specific legality logic is gone. A castling move is legal
    // iff making it doesn't leave the mover's protected pieces (or their
    // E‑ghosts) capturable — same test as any other move.
    fn is_move_legal(
        &mut self,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        self.execute_expanded_move(mv, move_generator, config_manager);
        let is_safe = !self.mover_king_in_check(move_generator);
        self.undo_move(config_manager);
        is_safe
    }

    pub fn get_legal_moves(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Vec<ExpandedMove> {
        self.performance.lock().unwrap().legal_move_checks += 1;
        let mut moves = match self.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Moves(m) => m,
            MoveGenerationResult::Checkmate { .. } => return Vec::new(),
        };

        // ZERO-CLONE IN-PLACE FILTERING - Massively speeds up depth traversal checks!
        moves.retain(|mv| self.is_move_legal(mv, move_generator, config_manager));
        moves
    }

    pub fn execute_expanded_move(
        &mut self,
        mv: &ExpandedMove,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) {
        self.performance.lock().unwrap().moves_made += 1;
        if let Some(ref option) = mv.castling_option {
            self.execute_castling(
                mv.from,
                mv.to,
                &mv.move_with_path,
                option,
                config_manager,
                move_generator,
            );
        } else {
            self.make_move(
                mv.from,
                mv.to,
                &mv.move_with_path,
                config_manager,
                mv.promotion_target,
            );
        }
    }

    /// Would capturing this piece immediately end the game?
    ///
    /// 'R' pieces: always yes (that's what royal means).
    /// 'r' pieces: yes only if this is the LAST one of its color. The
    /// count is read from the board's incremental tracking *before* the
    /// capture — the victim is still present — so `count <= 1` means
    /// "this is the only one."
    ///
    /// O(1): two bit tests plus one array-length read.
    #[inline]
    fn is_fatal_capture(&self, captured: Piece) -> bool {
        if captured.is_royal {
            return true;
        }
        if captured.is_royalty {
            return self.board.royalty_count(captured.color) <= 1;
        }
        false
    }

    pub fn generate_pseudo_legal_moves(
        &self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> MoveGenerationResult {
        self.performance.lock().unwrap().pseudo_legal_generations += 1;
        let mut all_moves = Vec::new();
        let pieces = self.board.get_pieces_by_color(self.current_turn);

        for (from_pos, piece) in pieces {
            let moves_with_paths = move_generator.generate_moves_with_database(
                &self.board,
                from_pos,
                piece.piece_type,
            );
            for move_with_path in moves_with_paths {
                let to = move_with_path.destination;
                if move_with_path.rule.is_king_castle {
                    let castling_options = move_generator.get_castling_options(
                        &self.board,
                        from_pos,
                        to,
                        &move_with_path,
                    );
                    for option in castling_options {
                        let rook_captures = self.board.get_piece(option.rook_to);
                        let em = ExpandedMove {
                            from: from_pos,
                            to,
                            move_with_path: move_with_path.clone(),
                            castling_option: Some(option.clone()),
                            promotion_target: None,
                            captures: rook_captures,
                            captures_position: if rook_captures.is_some() {
                                Some(option.rook_to)
                            } else {
                                None
                            },
                        };
                        self.performance.lock().unwrap().moves_generated += 1;
                        if let Some(c) = rook_captures {
                            if self.is_fatal_capture(c) {
                                return MoveGenerationResult::Checkmate {
                                    move_that_captures_royal: em,
                                };
                            }
                        }
                        all_moves.push(em);
                    }
                } else {
                    // Null move: don't record the mover as captured.
                    let mut captures = if to == from_pos {
                        None
                    } else {
                        self.board.get_piece(to)
                    };
                    let mut captures_position = if captures.is_some() { Some(to) } else { None };
                    if captures.is_none() && to != from_pos {
                        if let Some(ep) = self.board.get_en_passant_target(to) {
                            if ep.capturable_by_all || move_with_path.rule.captures_en_passant {
                                captures = self.board.get_piece(ep.piece_position);
                                captures_position = Some(ep.piece_position);
                            }
                        }
                    }
                    let can_promote = config_manager
                        .get_piece_by_index(piece.piece_type)
                        .map_or(false, |c| c.properties.can_promote);
                    if can_promote && self.promotion_config.is_promotion_zone(to, piece.color) {
                        let targets = PromotionManager::get_promotion_targets(
                            piece.piece_type,
                            config_manager,
                        );
                        if !targets.is_empty() {
                            for &target in &targets {
                                let em = ExpandedMove {
                                    from: from_pos,
                                    to,
                                    move_with_path: move_with_path.clone(),
                                    castling_option: None,
                                    promotion_target: Some(target),
                                    captures,
                                    captures_position,
                                };
                                self.performance.lock().unwrap().moves_generated += 1;
                                if let Some(c) = captures {
                                    if self.is_fatal_capture(c) {
                                        return MoveGenerationResult::Checkmate {
                                            move_that_captures_royal: em,
                                        };
                                    }
                                }
                                all_moves.push(em);
                            }
                        } else {
                            let em = ExpandedMove {
                                from: from_pos,
                                to,
                                move_with_path: move_with_path.clone(),
                                castling_option: None,
                                promotion_target: None,
                                captures,
                                captures_position,
                            };
                            self.performance.lock().unwrap().moves_generated += 1;
                            if let Some(c) = captures {
                                if self.is_fatal_capture(c) {
                                    return MoveGenerationResult::Checkmate {
                                        move_that_captures_royal: em,
                                    };
                                }
                            }
                            all_moves.push(em);
                        }
                    } else {
                        let em = ExpandedMove {
                            from: from_pos,
                            to,
                            move_with_path,
                            castling_option: None,
                            promotion_target: None,
                            captures,
                            captures_position,
                        };
                        self.performance.lock().unwrap().moves_generated += 1;
                        if let Some(c) = captures {
                            if self.is_fatal_capture(c) {
                                return MoveGenerationResult::Checkmate {
                                    move_that_captures_royal: em,
                                };
                            }
                        }
                        all_moves.push(em);
                    }
                }
            }
        }
        MoveGenerationResult::Moves(all_moves)
    }

    pub fn attempt_move(
        &mut self,
        from: Position,
        to: Position,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> MoveAttemptResult {
        if !self.is_valid_turn(from) {
            return MoveAttemptResult::Invalid;
        }
        if let Some(piece) = self.board.get_piece(from) {
            if let Some(move_with_path) =
                move_generator.get_move_rule(&self.board, from, to, piece.piece_type)
            {
                if move_with_path.rule.is_king_castle {
                    let options =
                        move_generator.get_castling_options(&self.board, from, to, &move_with_path);
                    let mut legal_options = Vec::new();
                    for opt in options {
                        let em = ExpandedMove {
                            from,
                            to,
                            move_with_path: move_with_path.clone(),
                            castling_option: Some(opt.clone()),
                            promotion_target: None,
                            captures: self.board.get_piece(opt.rook_to),
                            captures_position: if self.board.get_piece(opt.rook_to).is_some() {
                                Some(opt.rook_to)
                            } else {
                                None
                            },
                        };
                        if self.is_move_legal(&em, move_generator, config_manager) {
                            legal_options.push(opt);
                        }
                    }
                    if legal_options.is_empty() {
                        return MoveAttemptResult::Invalid;
                    }
                    if legal_options.len() == 1 {
                        self.redo_stack.clear();
                        self.execute_castling(
                            from,
                            to,
                            &move_with_path,
                            &legal_options[0],
                            config_manager,
                            move_generator,
                        );
                        return MoveAttemptResult::Success;
                    } else {
                        self.pending_move = Some(PendingMove::Castling {
                            king_from: from,
                            king_to: to,
                            king_move: move_with_path,
                            options: legal_options,
                        });
                        return MoveAttemptResult::NeedsCastlingChoice;
                    }
                }

                let mut captures = self.board.get_piece(to);
                let mut captures_position = if captures.is_some() { Some(to) } else { None };
                if captures.is_none() {
                    if let Some(ep) = self.board.get_en_passant_target(to) {
                        if ep.capturable_by_all || move_with_path.rule.captures_en_passant {
                            captures = self.board.get_piece(ep.piece_position);
                            captures_position = Some(ep.piece_position);
                        }
                    }
                }

                let can_promote = config_manager
                    .get_piece_by_index(piece.piece_type)
                    .map_or(false, |c| c.properties.can_promote);
                if can_promote && self.promotion_config.is_promotion_zone(to, piece.color) {
                    let targets =
                        PromotionManager::get_promotion_targets(piece.piece_type, config_manager);
                    if !targets.is_empty() {
                        let promotion_type =
                            RandomPromotionSelector.select_promotion(&targets, config_manager);
                        let em = ExpandedMove {
                            from,
                            to,
                            move_with_path: move_with_path.clone(),
                            castling_option: None,
                            promotion_target: promotion_type,
                            captures,
                            captures_position,
                        };
                        if !self.is_move_legal(&em, move_generator, config_manager) {
                            return MoveAttemptResult::Invalid;
                        }
                        self.redo_stack.clear();
                        self.make_move(from, to, &move_with_path, config_manager, promotion_type);
                        return MoveAttemptResult::Success;
                    }
                }

                let em = ExpandedMove {
                    from,
                    to,
                    move_with_path: move_with_path.clone(),
                    castling_option: None,
                    promotion_target: None,
                    captures,
                    captures_position,
                };
                if !self.is_move_legal(&em, move_generator, config_manager) {
                    return MoveAttemptResult::Invalid;
                }
                self.redo_stack.clear();
                self.make_move(from, to, &move_with_path, config_manager, None);
                return MoveAttemptResult::Success;
            }
        }
        MoveAttemptResult::Invalid
    }

    pub fn execute_castling(
        &mut self,
        king_from: Position,
        king_to: Position,
        king_move: &MoveWithPath,
        option: &CastlingOption,
        config_manager: &PieceConfigManager,
        _move_generator: &MoveGenerator,
    ) {
        let en_passant_targets_before = self.board.get_en_passant_targets().to_vec();
        let fifty_move_counter_before = self.fifty_move_counter;
        let castling_rights_before = self.castling_rights.clone();
        let rook_capture = self.board.get_piece(option.rook_to);

        let piece_hash_before = self.board.piece_hash();

        self.board.clear_en_passant_targets();

        let mut king = self.board.get_piece(king_from).unwrap();
        let mut rook = self.board.get_piece(option.rook_from).unwrap();

        self.board.set_piece(king_from, None);
        self.board.set_piece(option.rook_from, None);

        king.move_count += 1;
        rook.move_count += 1;

        self.board.set_piece(king_to, Some(king));
        self.board.set_piece(option.rook_to, Some(rook));

        // King promotion after castling. Both flags must be updated so
        // set_piece maintains royal_positions AND royalty_positions.
        let mut king_promoted_to = None;
        let king_promoted_from = if let Some(kp) = self.board.get_piece(king_to) {
            if PromotionManager::can_promote(kp.piece_type, config_manager)
                && self.promotion_config.is_promotion_zone(king_to, kp.color)
            {
                let targets =
                    PromotionManager::get_promotion_targets(kp.piece_type, config_manager);
                if !targets.is_empty() {
                    if let Some(nt) =
                        RandomPromotionSelector.select_promotion(&targets, config_manager)
                    {
                        let ot = kp.piece_type;
                        if let Some(mut p) = self.board.get_piece(king_to) {
                            let (r, ry) = piece_flags(nt, config_manager);
                            p.piece_type = nt;
                            p.is_royal = r;
                            p.is_royalty = ry;
                            self.board.set_piece(king_to, Some(p));
                        }
                        king_promoted_to = Some(nt);
                        Some(ot)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Rook promotion after castling — same treatment.
        let mut rook_promoted_to = None;
        let rook_promoted_from = if let Some(rp) = self.board.get_piece(option.rook_to) {
            if PromotionManager::can_promote(rp.piece_type, config_manager)
                && self
                    .promotion_config
                    .is_promotion_zone(option.rook_to, rp.color)
            {
                let targets =
                    PromotionManager::get_promotion_targets(rp.piece_type, config_manager);
                if !targets.is_empty() {
                    if let Some(nt) =
                        RandomPromotionSelector.select_promotion(&targets, config_manager)
                    {
                        let ot = rp.piece_type;
                        if let Some(mut p) = self.board.get_piece(option.rook_to) {
                            let (r, ry) = piece_flags(nt, config_manager);
                            p.piece_type = nt;
                            p.is_royal = r;
                            p.is_royalty = ry;
                            self.board.set_piece(option.rook_to, Some(p));
                        }
                        rook_promoted_to = Some(nt);
                        Some(ot)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        self.castling_rights.update_position(king_from, king_to);
        self.castling_rights
            .update_position(option.rook_from, option.rook_to);
        self.castling_rights.remove_rights(king_to);
        self.castling_rights.remove_rights(option.rook_to);
        self.create_en_passant_targets_from_path(king_to, king_move);
        if rook_capture.is_some() || king_promoted_from.is_some() || rook_promoted_from.is_some() {
            self.fifty_move_counter = 0;
        } else {
            self.fifty_move_counter += 1;
        }

        self.move_history.push(GameMove {
            from: king_from,
            to: king_to,
            captured_piece: None,
            fifty_move_counter_before_move: fifty_move_counter_before,
            en_passant_targets_before,
            captured_en_passant: None,
            captured_en_passant_piece: None,
            castling_rook_move: Some((option.rook_from, option.rook_to)),
            castling_rook_capture: rook_capture,
            castling_rights_before,
            promoted_from: king_promoted_from.or(rook_promoted_from),
            promoted_to: king_promoted_to.or(rook_promoted_to),
            piece_hash_before,
        });
        self.current_turn = self.current_turn.opposite();
        self.update_game_state(config_manager);
    }

    #[inline(always)]
    pub fn current_hash(&self) -> u64 {
        self.board.piece_hash() ^ turn_key(self.current_turn)
    }

    #[inline]
    pub fn calculate_current_hash(&self, _config_manager: &PieceConfigManager) -> u64 {
        self.current_hash()
    }

    pub fn is_valid_turn(&self, from: Position) -> bool {
        self.board
            .get_piece(from)
            .map_or(false, |p| p.color == self.current_turn)
    }

    pub fn make_move(
        &mut self,
        from: Position,
        to: Position,
        move_with_path: &MoveWithPath,
        config_manager: &PieceConfigManager,
        promotion_type: Option<usize>,
    ) {
        let en_passant_targets_before = self.board.get_en_passant_targets().to_vec();
        let castling_rights_before = self.castling_rights.clone();
        // Null/pass move (`?`): from == to. The mover is *not* a capture of
        // itself, and we mustn't let Board::move_piece see an aliased src/dst.
        let is_null = from == to;
        let captured_piece = if is_null {
            None
        } else {
            self.board.get_piece(to)
        };
        let piece_hash_before = self.board.piece_hash();

        let mut captured_en_passant = None;
        let mut captured_en_passant_piece = None;
        if !is_null {
            if let Some(ep) = self.board.get_en_passant_target(to).copied() {
                if ep.capturable_by_all || move_with_path.rule.captures_en_passant {
                    captured_en_passant_piece = self.board.get_piece(ep.piece_position);
                    if captured_en_passant_piece.is_some() {
                        self.board.set_piece(ep.piece_position, None);
                        captured_en_passant = Some(ep.piece_position);
                    }
                }
            }
        }
        let fifty_move_counter_before = self.fifty_move_counter;
        self.board.clear_en_passant_targets();

        if is_null {
            // Bump move_count in place so `u`-gated moves behave, without
            // risking a self-clobber inside Board::move_piece.
            if let Some(mut p) = self.board.get_piece(from) {
                p.move_count += 1;
                self.board.set_piece(from, Some(p));
            }
        } else {
            self.board.move_piece(from, to);
        }

        let promoted_from = if let Some(new_type) = promotion_type {
            if let Some(mut piece) = self.board.get_piece(to) {
                let original = piece.piece_type;
                let (r, ry) = piece_flags(new_type, config_manager);
                piece.piece_type = new_type;
                piece.is_royal = r;
                piece.is_royalty = ry;
                self.board.set_piece(to, Some(piece));
                Some(original)
            } else {
                None
            }
        } else {
            None
        };

        self.create_en_passant_targets_from_path(to, move_with_path);
        if captured_piece.is_some()
            || captured_en_passant_piece.is_some()
            || promoted_from.is_some()
        {
            self.fifty_move_counter = 0;
        } else {
            self.update_fifty_move_counter(false, &move_with_path.rule);
        }
        self.castling_rights.update_position(from, to);

        self.move_history.push(GameMove {
            from,
            to,
            captured_piece,
            fifty_move_counter_before_move: fifty_move_counter_before,
            en_passant_targets_before,
            captured_en_passant,
            captured_en_passant_piece,
            castling_rook_move: None,
            castling_rook_capture: None,
            castling_rights_before,
            promoted_from,
            promoted_to: promotion_type,
            piece_hash_before,
        });
        self.current_turn = self.current_turn.opposite();
        self.update_game_state(config_manager);
    }

    // ─── REVERT to original semantics ────────────────────────────────────────
    //
    // `path.steps[i]` is the square the piece stood on *before* hop
    // `step_indices[i]` fires. An `e`/`E` flag on that hop therefore marks the
    // *departure* square. The origin (i = 0) is intentionally included: for a
    // royal castler, the ghost it leaves on its starting square is what encodes
    // "cannot castle out of check" once mover_king_in_check is ghost‑aware.
    // For a non‑royal castler it's what lets the opponent take it in passing.
    // Do not "fix" this by skipping i == 0 or occupied squares.
    fn create_en_passant_targets_from_path(
        &mut self,
        final_position: Position,
        move_with_path: &MoveWithPath,
    ) {
        for (i, &pos_u8) in move_with_path.path.steps.iter().enumerate() {
            let pos = (pos_u8.0 as usize, pos_u8.1 as usize);
            if pos == final_position {
                continue;
            }
            if let Some(&step_idx) = move_with_path.path.step_indices.get(i) {
                if let Some(step) = move_with_path.rule.steps.get(step_idx as usize) {
                    if let Some(capturable_by_all) = step.creates_en_passant {
                        self.board.add_en_passant_target(EnPassantTarget {
                            position: pos,
                            capturable_by_all,
                            piece_position: final_position,
                        });
                    }
                }
            }
        }
    }

    fn update_fifty_move_counter(&mut self, is_capture: bool, rule: &CompiledMove) {
        if is_capture || rule.is_irreversible {
            self.fifty_move_counter = 0;
        } else {
            self.fifty_move_counter += 1;
        }
    }

    fn update_game_state(&mut self, _config_manager: &PieceConfigManager) {
        self.update_position_history();
        self.check_draw_conditions();
    }

    fn update_position_history(&mut self) {
        let h = self.current_hash();
        *self.position_history.entry(h).or_insert(0) += 1;
    }

    fn check_draw_conditions(&mut self) {
        if self.fifty_move_counter >= 100 {
            self.game_result = Some(GameResult::Draw(DrawReason::FiftyMoveRule));
            return;
        }
        let h = self.current_hash();
        if let Some(&count) = self.position_history.get(&h) {
            if count >= 3 {
                self.game_result = Some(GameResult::Draw(DrawReason::Repetition));
                return;
            }
        }
        // Dead-material check. The two guards inside is_draw() make this
        // effectively free everywhere except deep endgames, where it
        // *saves* time by letting Search return 0 for the whole subtree.
        // Ordered after the clock-based draws so those get reported with
        // their specific reason when both apply.
        if self.insufficient_material.is_draw(&self.board) {
            self.game_result = Some(GameResult::Draw(DrawReason::InsufficientMaterial));
            return;
        }
        self.game_result = Some(GameResult::Ongoing);
    }

    pub fn undo_move(&mut self, config_manager: &PieceConfigManager) -> bool {
        if let Some(last_move) = self.move_history.pop() {
            self.performance.lock().unwrap().moves_undone += 1;
            self.revert_position_history();
            self.revert_board_state(last_move, config_manager);
            true
        } else {
            false
        }
    }

    pub fn undo_move_for_gui(&mut self, config_manager: &PieceConfigManager) -> bool {
        if let Some(gm) = self.move_history.last().cloned() {
            self.redo_stack.push(gm);
        }
        self.undo_move(config_manager)
    }

    pub fn redo_move(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> bool {
        if let Some(redo) = self.redo_stack.pop() {
            let piece = match self.board.get_piece(redo.from) {
                Some(p) => p,
                None => return false,
            };
            if let Some((rook_from, _rook_to)) = redo.castling_rook_move {
                if let Some(mwp) =
                    move_generator.get_move_rule(&self.board, redo.from, redo.to, piece.piece_type)
                {
                    let options =
                        move_generator.get_castling_options(&self.board, redo.from, redo.to, &mwp);
                    if let Some(opt) = options.into_iter().find(|o| o.rook_from == rook_from) {
                        self.execute_castling(
                            redo.from,
                            redo.to,
                            &mwp,
                            &opt,
                            config_manager,
                            move_generator,
                        );
                        if let (Some(pt), Some(pf)) = (redo.promoted_to, redo.promoted_from) {
                            if let Some(last) = self.move_history.last_mut() {
                                last.promoted_to = Some(pt);
                                last.promoted_from = Some(pf);
                            }
                            if let Some(mut p) = self.board.get_piece(redo.to) {
                                if p.piece_type != pt {
                                    let (r, ry) = piece_flags(pt, config_manager);
                                    p.piece_type = pt;
                                    p.is_royal = r;
                                    p.is_royalty = ry;
                                    self.board.set_piece(redo.to, Some(p));
                                }
                            }
                        }
                        return true;
                    }
                }
                return false;
            } else {
                if let Some(mwp) =
                    move_generator.get_move_rule(&self.board, redo.from, redo.to, piece.piece_type)
                {
                    self.make_move(redo.from, redo.to, &mwp, config_manager, redo.promoted_to);
                    return true;
                }
                return false;
            }
        }
        false
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
    pub fn clear_redo(&mut self) {
        self.redo_stack.clear();
    }

    fn revert_position_history(&mut self) {
        let h = self.current_hash();
        if let Some(count) = self.position_history.get_mut(&h) {
            if *count > 1 {
                *count -= 1;
            } else {
                self.position_history.remove(&h);
            }
        }
    }

    fn revert_board_state(&mut self, last_move: GameMove, config_manager: &PieceConfigManager) {
        let mut piece = self.board.get_piece(last_move.to).unwrap();
        piece.move_count -= 1;

        // Un-promotion: restore both cached flags from the original type.
        if let Some(original_type) = last_move.promoted_from {
            let (r, ry) = piece_flags(original_type, config_manager);
            piece.piece_type = original_type;
            piece.is_royal = r;
            piece.is_royalty = ry;
        }

        let mut rook = None;
        if let Some((rook_from, rook_to)) = last_move.castling_rook_move {
            if let Some(mut r) = self.board.get_piece(rook_to) {
                r.move_count -= 1;
                rook = Some((rook_from, r));
            }
        }

        self.board.set_piece(last_move.to, None);
        if let Some((_, rook_to)) = last_move.castling_rook_move {
            self.board.set_piece(rook_to, None);
        }

        self.board.set_piece(last_move.from, Some(piece));

        if let Some((rook_from, r)) = rook {
            self.board.set_piece(rook_from, Some(r));
        }

        if let Some((_, rook_to)) = last_move.castling_rook_move {
            if let Some(captured) = last_move.castling_rook_capture {
                self.board.set_piece(rook_to, Some(captured));
            }
        }

        if let Some(captured) = last_move.captured_piece {
            self.board.set_piece(last_move.to, Some(captured));
        }

        if let Some(ep_pos) = last_move.captured_en_passant {
            if let Some(ep_piece) = last_move.captured_en_passant_piece {
                self.board.set_piece(ep_pos, Some(ep_piece));
            }
        }

        self.board
            .set_en_passant_targets(last_move.en_passant_targets_before);
        self.castling_rights
            .clone_from(&last_move.castling_rights_before);
        self.current_turn = self.current_turn.opposite();
        self.fifty_move_counter = last_move.fifty_move_counter_before_move;
        self.game_result = Some(GameResult::Ongoing);

        self.board.set_piece_hash(last_move.piece_hash_before);
    }

    /// Clone for use on a worker thread. Identical to `clone()` except the
    /// performance tracker is replaced with a fresh, unshared one so that
    /// parallel workers don't contend on a single `Mutex` every time they
    /// generate moves.
    pub fn clone_for_worker(&self) -> Self {
        let mut s = self.clone();
        s.performance = Arc::new(Mutex::new(PerformanceTracker::new()));
        s
    }

    pub fn can_undo(&self) -> bool {
        !self.move_history.is_empty()
    }
    pub fn get_position_string(&self, config_manager: &PieceConfigManager) -> String {
        self.board.to_position_string(config_manager)
    }
    pub fn move_count(&self) -> usize {
        self.move_history.len()
    }
    pub fn is_ongoing(&self) -> bool {
        matches!(self.game_result, Some(GameResult::Ongoing))
    }
}
