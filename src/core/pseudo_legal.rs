//src/core/pseudo_legal.rs
//! Pseudo-legal move generation and legal-move filtering.
//!
//! ── Allocation budget ────────────────────────────────────────────────────
//!
//! This used to allocate `1 + N` `Vec`s per node: one from
//! `Board::get_pieces_by_color` and one per piece from
//! `MoveGenerator::generate_moves_with_database`. At ~16 pieces that is 17
//! mallocs on the single hottest path in the engine.
//!
//! It now allocates **two**: the output `Vec<ExpandedMove>` and one scratch
//! buffer reused across every piece. The board is scanned in place.

use crate::core::game_state::GameState;
use crate::core::game_types::*;
use crate::move_generator::{flight_captures, resolve_landing_capture, MoveGenerator, MoveWithPath};
use crate::piece_config::PieceConfigManager;
use crate::promotion::PromotionManager;
use smallvec::SmallVec;

impl GameState {
    pub fn generate_pseudo_legal_moves(
        &self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> MoveGenerationResult {
        PerformanceTracker::inc(&self.performance.pseudo_legal_generations);

        let (rows, cols) = self.board.size();
        let turn = self.current_turn;
        let mut all_moves: Vec<ExpandedMove> = Vec::with_capacity(48);
        let mut buf: Vec<MoveWithPath> = Vec::with_capacity(32);

        for row in 0..rows {
            for col in 0..cols {
                let from_pos = (row, col);
                let Some(piece) = self.board.get_piece(from_pos) else { continue };
                if piece.color != turn {
                    continue;
                }

                buf.clear();
                move_generator.generate_moves_into(&self.board, from_pos, piece.piece_type, &mut buf);

                for mwp in buf.drain(..) {
                    let to = mwp.destination;

                    // ── Castling expansion ──────────────────────────
                    if mwp.rule.is_king_castle {
                        let options =
                        move_generator.get_castling_options(&self.board, from_pos, to, &mwp);
                        let king_flight = flight_captures(&self.board, &mwp, piece.color);

                        for option in options {
                            let rook_captures = self.board.get_piece(option.rook_to);
                            let em = ExpandedMove {
                                from: from_pos,
                                to,
                                move_with_path: mwp.clone(),
                                castling_option: Some(option.clone()),
                                promotion_target: None,
                                captures: rook_captures,
                                captures_position: rook_captures.map(|_| option.rook_to),
                            };
                            PerformanceTracker::inc(&self.performance.moves_generated);
                            if self.is_fatal_capture_for(piece.color, rook_captures, &king_flight) {
                                return MoveGenerationResult::Checkmate { move_that_captures_royal: em };
                            }
                            all_moves.push(em);
                        }
                        continue;
                    }

                    // ── Capture resolution: ONE rule, shared with MoveChain ──
                    let is_null = to == from_pos;
                    let (captures, captures_position) = if is_null {
                        (None, None)
                    } else {
                        match resolve_landing_capture(&self.board, to, &mwp.rule) {
                            Some((victim, sq)) => (Some(victim), Some(sq)),
                            None => (None, None),
                        }
                    };

                    let flight = if is_null {
                        SmallVec::new()
                    } else {
                        flight_captures(&self.board, &mwp, piece.color)
                    };

                    // ── Promotion expansion ─────────────────────────
                    let can_promote = config_manager
                    .get_piece_by_index(piece.piece_type)
                    .map_or(false, |c| c.properties.can_promote);
                    let in_zone =
                    can_promote && self.promotion_config.is_promotion_zone(to, piece.color);

                    if in_zone {
                        let targets =
                        PromotionManager::get_promotion_targets(piece.piece_type, config_manager);
                        if !targets.is_empty() {
                            for &target in &targets {
                                let em = ExpandedMove {
                                    from: from_pos,
                                    to,
                                    move_with_path: mwp.clone(),
                                    castling_option: None,
                                    promotion_target: Some(target),
                                    captures,
                                    captures_position,
                                };
                                PerformanceTracker::inc(&self.performance.moves_generated);
                                if self.is_fatal_capture_for(piece.color, captures, &flight) {
                                    return MoveGenerationResult::Checkmate { move_that_captures_royal: em };
                                }
                                all_moves.push(em);
                            }
                            continue;
                        }
                    }

                    let em = ExpandedMove {
                        from: from_pos,
                        to,
                        move_with_path: mwp,
                        castling_option: None,
                        promotion_target: None,
                        captures,
                        captures_position,
                    };
                    PerformanceTracker::inc(&self.performance.moves_generated);
                    if self.is_fatal_capture_for(piece.color, captures, &flight) {
                        return MoveGenerationResult::Checkmate { move_that_captures_royal: em };
                    }
                    all_moves.push(em);
                }
            }
        }

        MoveGenerationResult::Moves(all_moves)
    }

    pub fn get_legal_moves(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> Vec<ExpandedMove> {
        PerformanceTracker::inc(&self.performance.legal_move_checks);

        let mut moves = match self.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Moves(m) => m,
            MoveGenerationResult::Checkmate { .. } => return Vec::new(),
        };

        moves.retain(|mv| self.is_move_legal(mv, move_generator, config_manager));
        moves
    }

    /// Count legal moves without materialising them. Used by perft's bulk
    /// leaf counting, where building 30 `ExpandedMove`s only to drop them is
    /// pure waste.
    pub fn count_legal_moves(
        &mut self,
        move_generator: &MoveGenerator,
        config_manager: &PieceConfigManager,
    ) -> u64 {
        let moves = match self.generate_pseudo_legal_moves(move_generator, config_manager) {
            MoveGenerationResult::Moves(m) => m,
            MoveGenerationResult::Checkmate { .. } => return 0,
        };
        let mut n = 0u64;
        for mv in &moves {
            self.execute_expanded_move(mv, move_generator, config_manager);
            if !self.mover_king_in_check(move_generator) {
                n += 1;
            }
            self.undo_move(config_manager);
        }
        n
    }
}
